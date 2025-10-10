use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Result;
use cargo_metadata::camino::Utf8PathBuf;
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::{ProjectJson, util::FilePathBuf};

#[derive(PartialEq, Clone, Debug, Parser)]
pub struct CargoSubspace {
    /// Enables verbose logging.
    #[arg(long, short)]
    pub verbose: bool,

    /// Emit logs to stdout.
    ///
    /// NOTE: Including this flag will break rust-analyzer!
    #[arg(long)]
    pub log_to_stdout: bool,

    /// The explicit path to the directory containing your cargo binaries. By default,
    /// `cargo-subspace` will use the binaries on your `PATH`.
    #[arg(long, env = "CARGO_HOME")]
    pub cargo_home: Option<PathBuf>,

    #[arg(long, hide = true)]
    pub flamegraph: Option<PathBuf>,

    /// The location where log files will be stored.
    ///
    /// Default: $HOME/.local/state/cargo-subspace/cargo-subspace.log
    #[arg(long)]
    pub log_location: Option<PathBuf>,

    #[command(subcommand)]
    pub command: SubspaceCommand,
}

#[derive(PartialEq, Clone, Debug, Parser)]
pub enum SubspaceCommand {
    /// Print the cargo-subspace version and sysroot path and exit
    Version,
    Discover {
        arg: DiscoverArgument,
    },
    Check {
        path: FilePathBuf,

        /// Disables the emission of ANSI color codes in diagnostic output. Useful if your editor
        /// doesn't correctly render ANSI color codes.
        #[arg(long)]
        disable_color_diagnostics: bool,
    },
    Clippy {
        path: FilePathBuf,

        /// Disables the emission of ANSI color codes in diagnostic output. Useful if your editor
        /// doesn't correctly render ANSI color codes.
        #[arg(long)]
        disable_color_diagnostics: bool,
    },
}

#[derive(PartialEq, Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscoverArgument {
    Path(FilePathBuf),
    Buildfile(FilePathBuf),
}

impl FromStr for DiscoverArgument {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|_| {
            anyhow::anyhow!(
                "Expected a JSON object with a key of `path`, `buildfile`, or `label`. Got: {}",
                s
            )
            .context("Failed to deserialize argument to `discover`")
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
#[serde(rename_all = "snake_case")]
pub enum DiscoverProjectData {
    Finished {
        buildfile: Utf8PathBuf,
        project: ProjectJson,
    },
    #[allow(unused)]
    Error {
        error: String,
        source: Option<String>,
    },
    #[allow(unused)]
    Progress { message: String },
}
