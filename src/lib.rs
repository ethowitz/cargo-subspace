mod cli;
pub mod discover;
pub mod graph;
mod rust_project;
mod util;

use std::env;
use std::fs::{self, File};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Result, anyhow};
use clap::Parser;
use rust_project::ProjectJson;
use tracing::level_filters::LevelFilter;
use tracing::{debug, error, info};
use tracing_appender::non_blocking::WorkerGuard;

use cli::{CargoSubspace, CheckArgs, DiscoverArgument, DiscoverProjectData, SubspaceCommand};
use discover::DiscoverRunner;
use util::FilePathBuf;

const DEFAULT_LOG_LOCATION: &str = ".local/state/cargo-subspace";
const LOG_FILE_NAME: &str = "cargo-subspace.log";

pub fn run() -> Result<()> {
    let command = env::args().collect::<Vec<_>>();
    let args = CargoSubspace::parse();

    let _tracing_guard = set_up_tracing(args.log_location.clone(), args.verbose)?;
    let version = version();

    let path = env::var("PATH")?;
    let dir = env::current_dir()?;
    debug!(path, cwd = %dir.display(), %version, ?command, ?args);

    main_inner(args.command, args.cargo_home).inspect_err(|e| {
        error!("{e}");

        let error = DiscoverProjectData::Error {
            error: e.to_string(),
            source: None,
        };

        println!("{}", serde_json::to_string(&error).unwrap());
    })
}

fn log_progress<T>(message: T) -> Result<()>
where
    T: Into<String>,
{
    let message = message.into();

    if is_tty() {
        info!("{message}");
    } else {
        let progress = DiscoverProjectData::Progress { message };
        println!("{}", serde_json::to_string(&progress)?);
    }

    Ok(())
}

fn toolchain_command(command: &str, cargo_home: Option<&Path>) -> Command {
    if let Some(cargo_home) = cargo_home {
        Command::new(cargo_home.join("bin").join(command))
    } else {
        Command::new(command)
    }
}

fn main_inner(command: SubspaceCommand, cargo_home: Option<PathBuf>) -> Result<()> {
    let execution_start = Instant::now();

    match command {
        SubspaceCommand::Version => {
            println!("{}", version());
        }
        SubspaceCommand::Discover {
            all_features,
            no_default_features,
            mut flamegraph,
            arg,
        } => {
            #[cfg(not(target_os = "windows"))]
            let pprof_guard = {
                flamegraph
                    .take()
                    .map(|path| {
                        Ok::<_, anyhow::Error>((
                            pprof::ProfilerGuardBuilder::default()
                                .frequency(100000)
                                .blocklist(&["libc", "libgcc", "pthread", "vdso"])
                                .build()?,
                            path,
                        ))
                    })
                    .transpose()?
            };

            let mut runner = match arg {
                DiscoverArgument::Path(path) => DiscoverRunner::from_src_file(path)?,
                DiscoverArgument::Buildfile(manifest_path) => {
                    DiscoverRunner::from_manifest_path(manifest_path)
                }
            };

            runner = match (all_features, no_default_features) {
                (false, false) => runner.with_default_features(),
                (true, false) => runner.with_all_features(),
                (false, true) => runner.with_no_default_features(),
                (true, true) => unreachable!("disallowed by clap"),
            };

            // This gives us the pruned depedency graph
            let (project, buildfile) = runner.with_cargo_home(cargo_home).run()?;

            let output = DiscoverProjectData::Finished {
                buildfile: buildfile.to_path_buf(),
                project,
            };
            let json = if is_tty() {
                serde_json::to_string_pretty(&output)?
            } else {
                serde_json::to_string(&output)?
            };

            println!("{json}");

            #[cfg(not(target_os = "windows"))]
            if let Some((guard, path)) = pprof_guard {
                let report = guard.report().build()?;
                let file = std::fs::File::create(path)?;

                report.flamegraph(file)?;
            }
        }
        SubspaceCommand::Check { args } => check("check", args, cargo_home)?,
        SubspaceCommand::Clippy { args } => check("clippy", args, cargo_home)?,
    }

    debug!(execution_time_seconds = execution_start.elapsed().as_secs_f32());

    Ok(())
}

fn set_up_tracing(log_location: Option<PathBuf>, verbose: bool) -> Result<Option<WorkerGuard>> {
    if io::stdout().is_terminal() {
        let level = if verbose {
            LevelFilter::DEBUG
        } else {
            LevelFilter::INFO
        };

        tracing_subscriber::fmt().with_max_level(level).init();

        Ok(None)
    } else {
        #[cfg(not(target_os = "windows"))]
        let home: PathBuf = env::var("HOME")?.into();
        #[cfg(target_os = "windows")]
        let home: PathBuf = env::var("USERPROFILE")?.into();
        let log_location = log_location
            .clone()
            .unwrap_or_else(|| home.join(DEFAULT_LOG_LOCATION));

        let level = if verbose {
            LevelFilter::DEBUG
        } else {
            LevelFilter::WARN
        };

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

fn check(command: &'static str, args: CheckArgs, cargo_home: Option<PathBuf>) -> Result<()> {
    let manifest = find_manifest(args.path)?;
    let message_format = if is_tty() {
        "--message-format=human"
    } else if args.disable_color_diagnostics {
        "--message-format=json"
    } else {
        "--message-format=json-diagnostic-rendered-ansi"
    };

    let mut cmd = toolchain_command("cargo", cargo_home.as_deref());

    cmd.arg(command)
        .arg(message_format)
        .arg("--keep-going")
        .arg("--all-targets")
        .arg("--manifest-path")
        .arg(manifest.as_file_path())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    for arg in args.passthrough_args {
        cmd.arg(arg);
    }

    cmd.spawn()?.wait()?;
    Ok(())
}

pub fn find_manifest(path: FilePathBuf) -> Result<FilePathBuf> {
    log_progress("Looking for manifest path")?;

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

fn is_tty() -> bool {
    io::stdout().is_terminal()
}
