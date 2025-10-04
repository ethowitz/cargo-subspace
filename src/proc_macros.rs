use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::Result;
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::{Artifact, BuildScript, Message, PackageId};

use crate::log_progress;
use crate::rust_project::PackageNode;

pub(crate) fn build_compile_time_dependencies<P>(
    manifest_path: P,
    names: &HashMap<PackageId, PackageNode>,
) -> Result<(
    HashMap<PackageId, Utf8PathBuf>,
    HashMap<PackageId, BuildScript>,
)>
where
    P: AsRef<Path>,
{
    // TODO: check rust version to decide whether to use --compile-time-deps, which allows us to
    // only build proc macros/build scripts during this step instead of building the whole crate
    let child = Command::new("cargo")
        // .arg("+nightly")
        .arg("check")
        // .arg("--compile-time-deps")
        .arg("--quiet")
        .arg("--message-format")
        .arg("json")
        .arg("--keep-going")
        .arg("--manifest-path")
        .arg(manifest_path.as_ref())
        // .arg("-Zunstable-options")
        // .env("__CARGO_TEST_CHANNEL_OVERRIDE_DO_NOT_USE_THIS", "nightly")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    let mut dylibs = HashMap::new();
    let mut build_scripts = HashMap::new();

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

                    dylibs.insert(package_id, dylib);
                }
            }
            Message::BuildScriptExecuted(script) => {
                if let Some(pkg) = names.get(&script.package_id) {
                    log_progress(format!("build script {} run", pkg.name))?;
                } else {
                    log_progress("build script run")?;
                }

                build_scripts.insert(script.package_id.clone(), script);
            }
            _ => (),
        }
    }

    Ok((dylibs, build_scripts))
}

fn is_dylib(path: &Utf8PathBuf) -> bool {
    path.extension()
        .map(|ext| ["dylib", "so", "dll"].contains(&ext))
        .unwrap_or(false)
}
