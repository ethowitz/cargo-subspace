use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{Result, anyhow};
use cargo_metadata::{Artifact, Message, Metadata, MetadataCommand, camino::Utf8PathBuf};
use tracing::debug;

use crate::{graph::CrateGraph, log_progress, rust_project::ProjectJson, util::FilePathBuf};

pub struct DiscoverRunner {
    toolchain: Toolchain,
    features: FeatureOption,
    manifest_path: FilePathBuf,
}

impl DiscoverRunner {
    pub fn from_manifest_path(manifest_path: FilePathBuf) -> Self {
        Self {
            manifest_path,
            toolchain: Toolchain::default(),
            features: FeatureOption::Default,
        }
    }

    pub fn from_src_file(src_file: FilePathBuf) -> Result<Self> {
        let manifest_path = crate::find_manifest(src_file)?;
        Ok(Self::from_manifest_path(manifest_path))
    }

    pub fn with_all_features(mut self) -> Self {
        self.features = FeatureOption::All;
        self
    }

    pub fn with_no_default_features(mut self) -> Self {
        self.features = FeatureOption::NoDefault;
        self
    }

    pub fn with_default_features(mut self) -> Self {
        self.features = FeatureOption::Default;
        self
    }

    pub fn with_cargo_home(mut self, cargo_home: Option<PathBuf>) -> Self {
        self.toolchain = Toolchain { cargo_home };
        self
    }

    pub fn get_crate_graph(&self) -> Result<CrateGraph> {
        // Get the cargo workspace metadata
        let metadata = self.get_metadata()?;

        // Lower the metadata into our internal crate graph representation
        let mut graph = CrateGraph::from_metadata(metadata)?;

        // Prune the graph such that the remaining nodes are only those reachable from the node
        // with the given manifest path
        graph.prune(self.manifest_path.as_file_path())?;

        Ok(graph)
    }

    /// Fetches the cargo metadata, prunes the crate graph based on the given manifest path, and
    /// returns the graph in the format expected by rust-analyzer
    pub fn run(self) -> Result<(ProjectJson, FilePathBuf)> {
        let mut graph = self.get_crate_graph()?;

        // Build the compile time dependencies (proc macros & build scripts) for the pruned graph
        self.build_compile_time_dependencies(&mut graph)?;

        // Convert the crate graph into a flat list of crates
        let crates = graph.into_crates()?;

        let p: PathBuf = String::from_utf8(
            self.toolchain
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
        debug!(sysroot = %sysroot);

        let sysroot_src = sysroot.join("lib/rustlib/src/rust/library");

        let root = self
            .toolchain
            .cargo()
            .arg("locate-project")
            .arg("--workspace")
            .arg("--manifest-path")
            .arg(self.manifest_path.as_std_path())
            .arg("--message-format")
            .arg("plain")
            .output()?;
        let buildfile: PathBuf = String::from_utf8(root.stdout)?.trim().into();

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

        Ok((project, buildfile.try_into()?))
    }

    fn get_metadata(&self) -> Result<Metadata> {
        log_progress("Fetching metadata")?;

        let rustc_info = String::from_utf8(self.toolchain.rustc().arg("-vV").output()?.stdout)?;
        let mut cmd = MetadataCommand::new();
        cmd.manifest_path(self.manifest_path.as_std_path());

        if let Some(cargo_home) = self.toolchain.cargo_home.as_ref() {
            cmd.cargo_path(cargo_home.join("bin/cargo"));
        }

        let target_triple = rustc_info
            .lines()
            .find_map(|line| line.strip_prefix("host: "));
        if let Some(target_triple) = target_triple {
            cmd.other_options(["--filter-platform".into(), target_triple.into()]);
        }

        match self.features {
            FeatureOption::All => {
                cmd.features(cargo_metadata::CargoOpt::AllFeatures);
            }
            FeatureOption::NoDefault => {
                cmd.features(cargo_metadata::CargoOpt::NoDefaultFeatures);
            }
            FeatureOption::Default => (),
        }

        Ok(cmd.exec()?)
    }

    fn build_compile_time_dependencies(&self, graph: &mut CrateGraph) -> Result<()> {
        // TODO: check rust version to decide whether to use --compile-time-deps, which allows us to
        // only build proc macros/build scripts during this step instead of building the whole crate
        let child = self
            .toolchain
            .cargo()
            // .arg("+nightly")
            .arg("check")
            // .arg("--compile-time-deps")
            .arg("--quiet")
            .arg("--message-format")
            .arg("json")
            .arg("--keep-going")
            .arg("--all-targets")
            .arg("--manifest-path")
            .arg(self.manifest_path.as_std_path())
            // .arg("-Zunstable-options")
            // .env("__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS", "nightly")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        for line in BufReader::new(child.stdout.unwrap()).lines() {
            let line = line?;
            let message = serde_json::from_str::<Message>(&line)?;

            match message {
                Message::CompilerArtifact(Artifact {
                    filenames,
                    target,
                    package_id,
                    ..
                }) => {
                    if let Some(dylib) = filenames.into_iter().find(is_dylib)
                        && target.is_proc_macro()
                    {
                        log_progress(format!("proc-macro {} built", target.name))?;
                        if let Some(pkg) = graph.get_mut(&package_id) {
                            pkg.proc_macro_dylib = Some(dylib.try_into()?);
                        }
                    }
                }
                Message::BuildScriptExecuted(script) => {
                    if let Some(pkg) = graph.get_mut(&script.package_id) {
                        log_progress(format!("build script {} run", pkg.name))?;
                        pkg.build_script = Some(script);
                    }
                }
                _ => (),
            }
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FeatureOption {
    NoDefault,
    All,
    Default,
}

#[derive(Default)]
struct Toolchain {
    cargo_home: Option<PathBuf>,
}

impl Toolchain {
    fn cargo_command(&self, cmd: &str) -> Command {
        if let Some(cargo_home) = self.cargo_home.as_ref() {
            Command::new(cargo_home.join("bin").join(cmd))
        } else {
            Command::new(cmd)
        }
    }

    fn rustc(&self) -> Command {
        self.cargo_command("rustc")
    }

    fn cargo(&self) -> Command {
        self.cargo_command("cargo")
    }
}

fn is_dylib(path: &Utf8PathBuf) -> bool {
    path.extension()
        .map(|ext| ["dylib", "so", "dll"].contains(&ext))
        .unwrap_or(false)
}
