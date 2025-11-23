use std::{
    env,
    fs::{self, File},
    io::{self, IsTerminal},
    path::PathBuf,
    time::Instant,
};

use anyhow::{Result, anyhow};
use cargo_metadata::camino::Utf8PathBuf;
use cargo_subspace::{
    CONFIG_FILE_NAME,
    cli::{CargoSubspace, DiscoverArgument, DiscoverProjectData, SubspaceCommand},
    util::{self, Toolchain},
};
use cargo_subspace::{
    CheckCommand, DiscoverRunner, FeatureOption, ProjectJson, SubspaceConfig, check, find_manifest,
    get_metadata,
};
use clap::Parser;
use tracing::{debug, error, level_filters::LevelFilter};
use tracing_appender::non_blocking::WorkerGuard;

const DEFAULT_LOG_LOCATION: &str = ".local/state/cargo-subspace";
const LOG_FILE_NAME: &str = "cargo-subspace.log";

fn main() -> Result<()> {
    let command = env::args().collect::<Vec<_>>();
    let args = CargoSubspace::parse();

    let _tracing_guard = set_up_tracing(args.log_location.clone(), args.verbose)?;
    let version = version();

    let path = env::var("PATH")?;
    let dir = env::current_dir()?;
    debug!(path, cwd = %dir.display(), %version, ?command, ?args);

    run_inner(args.command, args.cargo_home).inspect_err(|e| {
        error!("{e}");

        let error = DiscoverProjectData::Error {
            error: e.to_string(),
            source: None,
        };

        println!("{}", serde_json::to_string(&error).unwrap());
    })
}

fn run_inner(command: SubspaceCommand, cargo_home: Option<PathBuf>) -> Result<()> {
    let execution_start = Instant::now();

    match command {
        SubspaceCommand::Version => {
            println!("{}", version());
        }
        SubspaceCommand::Discover {
            all_features,
            no_default_features,
            features,
            #[cfg(not(target_os = "windows"))]
            mut flamegraph,
            arg,
        } => {
            let toolchain = Toolchain::new(cargo_home);
            let subspace_config = SubspaceConfig::find(&toolchain)?;

            let features = match (all_features, no_default_features, features) {
                (false, false, None) => FeatureOption::Default,
                (true, false, None) => FeatureOption::All,
                (false, true, None) => FeatureOption::NoDefault,
                (false, false, Some(features)) => FeatureOption::Some(features),
                _ => unreachable!("disallowed by clap"),
            };
            let manifest_path = match arg {
                DiscoverArgument::Path(path) => find_manifest(path)?,
                DiscoverArgument::Buildfile(manifest_path) => manifest_path,
            };
            let metadata = get_metadata(
                &toolchain,
                Some(manifest_path.as_file_path()),
                features,
                false,
            )?;
            let crate_name = metadata
                .packages
                .iter()
                .find_map(|package| {
                    (package.manifest_path == manifest_path.as_std_path())
                        .then(|| package.name.to_string())
                })
                .unwrap_or_else(|| "unknown crate".into());
            // TODO: what is this config.workspace option actually doing?
            let buildfile = if subspace_config.workspace {
                metadata.workspace_root.join("Cargo.toml")
            } else {
                if !subspace_config.includes_package(&manifest_path.as_file_path()) {
                    let output = DiscoverProjectData::Error {
                        error: format!("Crate `{crate_name}` not included in {CONFIG_FILE_NAME}",),
                        source: None,
                    };
                    // TODO: dry this up
                    let json = if util::is_tty() {
                        serde_json::to_string_pretty(&output)?
                    } else {
                        serde_json::to_string(&output)?
                    };

                    println!("{json}");
                }

                manifest_path.to_path_buf()
            };

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

            let crates = DiscoverRunner::new(
                toolchain.clone(),
                manifest_path.clone(),
                subspace_config,
                crate_name,
                metadata,
            )
            .run()?
            .into_crates()?;

            let p: PathBuf = String::from_utf8(
                toolchain
                    .rustc()
                    .arg("--print")
                    .arg("sysroot")
                    .output()?
                    .stdout,
            )?
            .trim()
            .into();

            let sysroot = Utf8PathBuf::from_path_buf(p)
                .map_err(|_| anyhow!("Path contains non-UTF-8 characters"))?;
            let sysroot_src = sysroot.join("lib/rustlib/src/rust/library");

            let project = ProjectJson {
                sysroot,
                sysroot_src: Some(sysroot_src),
                // TODO: do i need this? buck excludes it...
                // sysroot_project: None,
                // TODO: do i need this? buck excludes it...
                // cfg_groups: HashMap::new(),
                crates,
                // TODO: Add support for runnables
                runnables: vec![],
            };

            let output = DiscoverProjectData::Finished { buildfile, project };
            let json = if util::is_tty() {
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
        SubspaceCommand::Check { args } => check(CheckCommand::Check, args, cargo_home)?,
        SubspaceCommand::Clippy { args } => check(CheckCommand::Clippy, args, cargo_home)?,
    }

    debug!(execution_time_seconds = execution_start.elapsed().as_secs_f32());

    Ok(())
}

fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
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
