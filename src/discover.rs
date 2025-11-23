use std::{
    io::{BufRead, BufReader},
    process::Stdio,
};

use anyhow::Result;
use cargo_metadata::{Artifact, Message, Metadata, camino::Utf8PathBuf};

use crate::{
    config::SubspaceConfig,
    graph::CrateGraph,
    util::{self, FilePathBuf, Toolchain},
};

pub struct DiscoverRunner {
    toolchain: Toolchain,
    subspace_config: SubspaceConfig,
    manifest_path: FilePathBuf,
    crate_name: String,
    metadata: Metadata,
}

impl DiscoverRunner {
    pub fn new(
        toolchain: Toolchain,
        manifest_path: FilePathBuf,
        subspace_config: SubspaceConfig,
        crate_name: String,
        metadata: Metadata,
    ) -> Self {
        Self {
            manifest_path,
            toolchain,
            subspace_config,
            crate_name,
            metadata,
        }
    }
    /// Fetches the cargo metadata, constructs a crate graph, and prunes the graph such that it
    /// only contains dependencies of the crate for the given manifest path
    pub fn run(self) -> Result<CrateGraph> {
        // Lower the metadata into our internal crate graph representation
        let mut graph = CrateGraph::from_metadata(self.metadata.clone())?;

        if self.subspace_config.uses_explicit_filters() {
            // If the workspace config uses explicit package filtering, we prune the graph using
            // the crates specific in the config
            graph.prune_with_config(&self.subspace_config)?;
        } else {
            // Prune the graph such that the remaining nodes are only those reachable from the node
            // with the given manifest path
            graph.prune(
                &self.subspace_config,
                &self.crate_name,
                self.manifest_path.as_file_path(),
            )?;
        }

        // Build the compile time dependencies (proc macros & build scripts) for the pruned graph
        self.build_compile_time_dependencies(&mut graph)?;

        Ok(graph)
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

fn is_dylib(path: &Utf8PathBuf) -> bool {
    path.extension()
        .map(|ext| ["dylib", "so", "dll"].contains(&ext))
        .unwrap_or(false)
}
