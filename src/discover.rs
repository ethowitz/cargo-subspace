use std::{
    io::{BufRead, BufReader},
    process::Stdio,
};

use anyhow::Result;
use cargo_metadata::{Artifact, Message, Metadata, MetadataCommand, camino::Utf8PathBuf};

use crate::{
    graph::CrateGraph,
    util::{self, FilePathBuf, Toolchain},
};

pub struct DiscoverRunner {
    toolchain: Toolchain,
    features: FeatureOption,
    manifest_path: FilePathBuf,
}

impl DiscoverRunner {
    pub fn new(toolchain: Toolchain, manifest_path: FilePathBuf) -> Self {
        Self {
            manifest_path,
            toolchain,
            features: FeatureOption::Default,
        }
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

    /// Fetches the cargo metadata, constructs a crate graph, and prunes the graph such that it
    /// only contains dependencies of the crate for the given manifest path
    pub fn run(self) -> Result<CrateGraph> {
        // Get the cargo workspace metadata
        let metadata = self.get_metadata()?;

        // Lower the metadata into our internal crate graph representation
        let mut graph = CrateGraph::from_metadata(metadata)?;

        // Prune the graph such that the remaining nodes are only those reachable from the node
        // with the given manifest path
        graph.prune(self.manifest_path.as_file_path())?;

        // Build the compile time dependencies (proc macros & build scripts) for the pruned graph
        self.build_compile_time_dependencies(&mut graph)?;

        Ok(graph)
    }

    fn get_metadata(&self) -> Result<Metadata> {
        util::log_progress("Fetching metadata")?;

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
                        util::log_progress(format!("proc-macro {} built", target.name))?;
                        if let Some(pkg) = graph.get_mut(&package_id) {
                            pkg.proc_macro_dylib = Some(dylib.try_into()?);
                        }
                    }
                }
                Message::BuildScriptExecuted(script) => {
                    if let Some(pkg) = graph.get_mut(&script.package_id) {
                        util::log_progress(format!("build script {} run", pkg.name))?;
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

fn is_dylib(path: &Utf8PathBuf) -> bool {
    path.extension()
        .map(|ext| ["dylib", "so", "dll"].contains(&ext))
        .unwrap_or(false)
}
