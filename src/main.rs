mod cli;
mod proc_macros;
mod rust_project;

use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
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

use crate::rust_project::{compute_project_json, find_sysroot};
use cli::{CargoSubspace, DiscoverArgument, DiscoverProjectData, SubspaceCommand};

const DEFAULT_LOG_LOCATION: &str = ".local/state/cargo-subspace";
const LOG_FILE_NAME: &str = "cargo-subspace.log";

fn main() -> Result<()> {
    let command = env::args().collect::<Vec<_>>();
    let args = CargoSubspace::parse();

    let _tracing_guard = set_up_tracing(&args)?;
    let version = version();
    let sysroot = find_sysroot()?;

    debug!(%version, sysroot = %sysroot.display(), ?command, ?args);
    main_inner(args).inspect_err(|e| {
        error!("{e}");

        let error = DiscoverProjectData::Error {
            error: e.to_string(),
            source: None,
        };

        println!("{}", serde_json::to_string(&error).unwrap());
    })
}

fn main_inner(args: CargoSubspace) -> Result<()> {
    let execution_start = Instant::now();

    match args.command {
        SubspaceCommand::Version => {
            println!("{}", version());
        }
        SubspaceCommand::Discover { arg } => match arg {
            DiscoverArgument::Path(path) => {
                log_progress("Looking for manifest path")?;
                let manifest_path = find_manifest(&path)?;

                discover(manifest_path, args.flamegraph)?;
            }
            DiscoverArgument::Buildfile(manifest_path) => discover(manifest_path, args.flamegraph)?,
        },
        SubspaceCommand::Check { path } => check("check", path)?,
        SubspaceCommand::Clippy { path } => check("clippy", path)?,
    }

    debug!(execution_time = execution_start.elapsed().as_secs_f32());

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

fn discover<P>(manifest_path: P, flamegraph: Option<PathBuf>) -> Result<()>
where
    P: AsRef<Path>,
{
    let manifest_path = manifest_path.as_ref();

    log_progress("Fetching packages")?;
    Command::new("cargo")
        .arg("fetch")
        .arg("--manifest-path")
        .arg(manifest_path.as_os_str())
        .output()?;

    log_progress("Fetching metadata")?;
    let metadata = MetadataCommand::new()
        .features(CargoOpt::AllFeatures)
        .exec()?;

    let project = compute_project_json(metadata, manifest_path, flamegraph)?;

    let workspace_root = Command::new("cargo")
        .arg("locate-project")
        .arg("--workspace")
        .arg("--message-format")
        .arg("plain")
        .output()?;
    let buildfile: PathBuf = String::from_utf8(workspace_root.stdout)?.into();
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

fn check(command: &'static str, file: String) -> Result<()> {
    let manifest = find_manifest(file)?;

    let status = Command::new("cargo")
        .arg(command)
        .arg("--message-format=json")
        .arg("--keep-going")
        .arg("--all-targets")
        .arg("--manifest-path")
        .arg(&manifest)
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

fn find_manifest<P>(path: P) -> Result<PathBuf>
where
    P: AsRef<Path>,
{
    let path = std::path::absolute(path)?;
    let Some(parent) = path.parent() else {
        anyhow::bail!("Invalid path: could not get parent");
    };

    for ancestor in parent.ancestors() {
        for item in std::fs::read_dir(ancestor)? {
            let item = item?;
            if item.file_type()?.is_file() && item.file_name() == "Cargo.toml" {
                let path = std::path::absolute(item.path())?;
                debug!(manifest_path = %path.display());

                return Ok(path);
            }
        }
    }

    Err(anyhow!(
        "Could not find manifest for path `{}`",
        path.display()
    ))
}
