pub mod cli;
mod config;
mod discover;
mod graph;
mod rust_project;
pub mod util;

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Result, anyhow};
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::{Metadata, MetadataCommand};
use tracing::debug;

use crate::cli::CheckArgs;
use crate::util::{FilePath, FilePathBuf, Toolchain};

pub use config::{CONFIG_FILE_NAME, CheckCommand, SubspaceConfig};
pub use discover::DiscoverRunner;
pub use rust_project::ProjectJson;

// TODO: tasks for this feature:
// - Add unit tests for SubspaceConfig pattern matching (make sure it's same as Cargo.toml logic)
// - Add documentation in readme
// - Actually use features lists from subspace config
// - Check how to handle default members?
// - Test all of this (come up with list of stuff to test before releases)

pub fn check(command: CheckCommand, args: CheckArgs, cargo_home: Option<PathBuf>) -> Result<()> {
    let toolchain = Toolchain::new(cargo_home);
    let metadata = get_metadata(&toolchain, None, FeatureOption::Default, true)?;
    let subspace_config = SubspaceConfig::find(&toolchain)?;

    let message_format = if util::is_tty() {
        "--message-format=human"
    } else if args.disable_color_diagnostics {
        "--message-format=json"
    } else {
        "--message-format=json-diagnostic-rendered-ansi"
    };

    let mut cmd = toolchain.cargo();
    let command = subspace_config.check.command.unwrap_or(command).to_string();
    let workspace = subspace_config.check.workspace.unwrap_or(args.workspace);

    cmd.arg(command.as_str())
        .arg(message_format)
        .arg("--keep-going")
        .arg("--all-targets")
        .current_dir(metadata.workspace_root.as_std_path())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    if workspace {
        let packages_by_id = metadata
            .packages
            .iter()
            .fold(HashMap::new(), |mut acc, pkg| {
                acc.insert(&pkg.id, pkg);
                acc
            });

        // Filter the workspace members such that we only keep those included by the workspace's
        // subspace config
        let filtered_ws_members = metadata.workspace_members.iter().filter_map(|id| {
            packages_by_id
                .get(id)
                .map(|pkg| {
                    subspace_config
                        .includes_package(&pkg.manifest_path)
                        .then_some(pkg)
                })
                .unwrap_or(None)
        });

        // Run a check command for the whole workspace, but only include the filtered workspace
        // members
        for pkg in filtered_ws_members {
            cmd.arg("-p").arg(pkg.name.to_string());
        }
    } else {
        let manifest = find_manifest(args.path.into())?;

        // TODO: look up crate in subspace_config and see if it has any features activated
        cmd.arg("--manifest-path").arg(manifest.as_file_path());

        let Some(name) = metadata.packages.iter().find_map(|pkg| {
            (pkg.manifest_path == manifest.as_std_path()).then_some(pkg.name.to_string())
        }) else {
            anyhow::bail!("internal: Could not find package with manifest path {manifest}");
        };

        if let Some(config) = subspace_config.crate_config.get(&name)
            && !config.features.is_empty()
        {
            let features = config.features.join(",");
            cmd.arg("--features").arg(features);
        }
    }

    // The passthrough args in the check config override ones passed directly to the binary
    let passthrough_args = subspace_config
        .check
        .passthrough_args
        .unwrap_or(args.passthrough_args);

    for arg in passthrough_args {
        cmd.arg(arg);
    }

    let status = cmd.spawn()?.wait()?;

    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("Failed to run check"))
    }
}

pub fn find_manifest(path: Utf8PathBuf) -> Result<FilePathBuf> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeatureOption {
    NoDefault,
    All,
    Default,
    Some(Vec<String>),
}

pub fn get_metadata(
    toolchain: &Toolchain,
    manifest_path: Option<FilePath<'_>>,
    features: FeatureOption,
    no_deps: bool,
) -> Result<Metadata> {
    util::log_progress("Fetching metadata")?;

    let rustc_info = String::from_utf8(toolchain.rustc().arg("-vV").output()?.stdout)?;
    let mut cmd = MetadataCommand::new();

    if let Some(manifest_path) = manifest_path {
        cmd.manifest_path(manifest_path.as_std_path());
    }

    if let Some(cargo_home) = toolchain.cargo_home.as_ref() {
        cmd.cargo_path(cargo_home.join("bin/cargo"));
    }

    let target_triple = rustc_info
        .lines()
        .find_map(|line| line.strip_prefix("host: "));
    if let Some(target_triple) = target_triple {
        cmd.other_options(["--filter-platform".into(), target_triple.into()]);
    }

    match features {
        FeatureOption::All => {
            cmd.features(cargo_metadata::CargoOpt::AllFeatures);
        }
        FeatureOption::NoDefault => {
            cmd.features(cargo_metadata::CargoOpt::NoDefaultFeatures);
        }
        FeatureOption::Some(features) => {
            cmd.features(cargo_metadata::CargoOpt::SomeFeatures(features));
        }
        FeatureOption::Default => (),
    }

    if no_deps {
        cmd.no_deps();
    }

    Ok(cmd.exec()?)
}
