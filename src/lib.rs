pub mod cli;
mod discover;
mod graph;
mod rust_project;
pub mod util;

use std::path::PathBuf;
use std::process::Stdio;

use anyhow::{Result, anyhow};
use cargo_metadata::camino::Utf8PathBuf;
use tracing::debug;

use crate::cli::CheckArgs;
use crate::util::{FilePathBuf, Toolchain};

pub use discover::DiscoverRunner;
pub use rust_project::ProjectJson;

pub fn check(command: &'static str, args: CheckArgs, cargo_home: Option<PathBuf>) -> Result<()> {
    let manifest = find_manifest(args.path.into())?;
    let message_format = if util::is_tty() {
        "--message-format=human"
    } else if args.disable_color_diagnostics {
        "--message-format=json"
    } else {
        "--message-format=json-diagnostic-rendered-ansi"
    };

    let mut cmd = Toolchain::new(cargo_home.clone()).cargo();

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
