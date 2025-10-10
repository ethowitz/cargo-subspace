mod cli;
mod proc_macros;
mod rust_project;
mod util;

use std::env;
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Result, anyhow};
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::{CargoOpt, MetadataCommand};
use clap::Parser;
use rust_project::ProjectJson;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error};
use tracing_appender::non_blocking::WorkerGuard;

use crate::rust_project::compute_project_json;
use crate::util::{FilePath, FilePathBuf};
use cli::{CargoSubspace, DiscoverArgument, DiscoverProjectData, SubspaceCommand};

const DEFAULT_LOG_LOCATION: &str = ".local/state/cargo-subspace";
const LOG_FILE_NAME: &str = "cargo-subspace.log";

fn main() -> Result<()> {
    let command = env::args().collect::<Vec<_>>();
    let args = CargoSubspace::parse();

    let _tracing_guard = set_up_tracing(&args)?;
    let version = version();

    let path = env::var("PATH")?;
    let dir = env::current_dir()?;
    debug!(path, cwd = %dir.display(), %version, ?command, ?args);

    main_inner(args).inspect_err(|e| {
        error!("{e}");

        let error = DiscoverProjectData::Error {
            error: e.to_string(),
            source: None,
        };

        println!("{}", serde_json::to_string(&error).unwrap());
    })
}

struct Context {
    cargo_home: Option<PathBuf>,
    flamegraph: Option<PathBuf>,
    disable_color_diagnostics: bool,
}

impl Context {
    fn cargo(&self) -> Command {
        self.toolchain_command("cargo")
    }

    fn rustc(&self) -> Command {
        self.toolchain_command("rustc")
    }

    fn toolchain_command(&self, command: &str) -> Command {
        let mut cmd = Command::new(command);

        if let Some(cargo_home) = self.cargo_home.as_ref() {
            cmd.env("PATH", cargo_home);
        }

        cmd
    }
}

impl From<&CargoSubspace> for Context {
    fn from(value: &CargoSubspace) -> Self {
        Context {
            cargo_home: value.cargo_home.clone(),
            flamegraph: value.flamegraph.clone(),
            disable_color_diagnostics: matches!(
                value.command,
                SubspaceCommand::Clippy {
                    disable_color_diagnostics: true,
                    ..
                } | SubspaceCommand::Check {
                    disable_color_diagnostics: true,
                    ..
                }
            ),
        }
    }
}

fn main_inner(args: CargoSubspace) -> Result<()> {
    let execution_start = Instant::now();
    let ctx = (&args).into();

    match args.command {
        SubspaceCommand::Version => {
            println!("{}", version());
        }
        SubspaceCommand::Discover { arg } => match arg {
            DiscoverArgument::Path(path) => {
                log_progress("Looking for manifest path")?;
                let manifest_path = find_manifest(path)?;

                discover(&ctx, manifest_path.as_file_path())?;
            }
            DiscoverArgument::Buildfile(manifest_path) => {
                discover(&ctx, manifest_path.as_file_path())?
            }
        },
        SubspaceCommand::Check { path, .. } => check(&ctx, "check", path)?,
        SubspaceCommand::Clippy { path, .. } => check(&ctx, "clippy", path)?,
    }

    debug!(execution_time_seconds = execution_start.elapsed().as_secs_f32());

    Ok(())
}

fn set_up_tracing(args: &CargoSubspace) -> Result<Option<WorkerGuard>> {
    let level = if args.verbose {
        LevelFilter::DEBUG
    } else {
        LevelFilter::WARN
    };

    if args.log_to_stdout {
        tracing_subscriber::fmt().with_max_level(level).init();

        Ok(None)
    } else {
        #[cfg(not(target_os = "windows"))]
        let home: PathBuf = env::var("HOME")?.into();
        #[cfg(target_os = "windows")]
        let home: PathBuf = env::var("USERPROFILE")?.into();
        let log_location = args
            .log_location
            .clone()
            .unwrap_or_else(|| home.join(DEFAULT_LOG_LOCATION));

        fs::create_dir_all(&log_location)?;

        let log_file = File::options()
            .append(true)
            .create(true)
            .open(log_location.join(LOG_FILE_NAME))?;

        let (non_blocking, guard) = tracing_appender::non_blocking(log_file);

        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(non_blocking)
            .with_max_level(level)
            .init();

        Ok(Some(guard))
    }
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn discover(ctx: &Context, manifest_path: FilePath<'_>) -> Result<()> {
    log_progress("Fetching packages")?;
    ctx.cargo()
        .arg("fetch")
        .arg("--manifest-path")
        .arg(manifest_path.as_os_str())
        .output()?;

    log_progress("Fetching metadata")?;
    let mut cmd = MetadataCommand::new();
    cmd.features(CargoOpt::AllFeatures)
        .manifest_path(manifest_path);

    if let Some(cargo_home) = ctx.cargo_home.as_ref() {
        cmd.cargo_path(cargo_home.join("cargo"));
    }

    let metadata = cmd.exec()?;

    let project = compute_project_json(ctx, metadata, manifest_path)?;

    let root = ctx
        .cargo()
        .arg("locate-project")
        .arg("--workspace")
        .arg("--manifest-path")
        .arg(manifest_path)
        .arg("--message-format")
        .arg("plain")
        .output()?;
    let buildfile: PathBuf = String::from_utf8(root.stdout)?.trim().into();
    let output = DiscoverProjectData::Finished {
        buildfile: Utf8PathBuf::from_path_buf(buildfile).map_err(|e| {
            anyhow!(
                "Manifest path `{}` contains non-UTF-8 characters",
                e.display()
            )
        })?,
        project,
    };
    let json = serde_json::to_string(&output)?;

    println!("{json}");

    Ok(())
}

fn check(ctx: &Context, command: &'static str, file: FilePathBuf) -> Result<()> {
    let manifest = find_manifest(file)?;
    let message_format = if ctx.disable_color_diagnostics {
        "--message-format=json"
    } else {
        "--message-format=json-diagnostic-rendered-ansi"
    };

    let status = ctx
        .cargo()
        .arg(command)
        .arg(message_format)
        .arg("--keep-going")
        .arg("--all-targets")
        .arg("--manifest-path")
        .arg(manifest.as_file_path())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?
        .wait()?;

    if status.success() {
        Ok(())
    } else {
        log_error(format!("Failed to run `cargo {command}`"))?;
        Err(anyhow!("Failed to run check"))
    }
}

fn log_progress<T>(message: T) -> Result<()>
where
    T: Into<String>,
{
    let message = message.into();

    let progress = DiscoverProjectData::Progress { message };
    println!("{}", serde_json::to_string(&progress)?);
    Ok(())
}

fn log_error<T>(message: T) -> Result<()>
where
    T: Into<String>,
{
    let progress = DiscoverProjectData::Error {
        error: message.into(),
        source: None,
    };
    println!("{}", serde_json::to_string(&progress)?);
    Ok(())
}

fn find_manifest(path: FilePathBuf) -> Result<FilePathBuf> {
    let path = std::path::absolute(&path)?;
    let Some(parent) = path.parent() else {
        anyhow::bail!("Invalid path: could not get parent");
    };

    for ancestor in parent.ancestors() {
        for item in std::fs::read_dir(ancestor)? {
            let item = item?;
            if item.file_type()?.is_file() && item.file_name() == "Cargo.toml" {
                let path = std::path::absolute(item.path())?;
                debug!(manifest_path = %path.display());

                return path.try_into();
            }
        }
    }

    Err(anyhow!(
        "Could not find manifest for path `{}`",
        path.display()
    ))
}
