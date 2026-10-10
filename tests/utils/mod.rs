// Shared by every test binary, and no single one uses all of the helpers
#![allow(dead_code)]

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use cargo_metadata::{PackageId, semver::Version};
use cargo_subspace::{
    DiscoverRunner,
    graph::{CrateGraph, PackageNode},
    util::Toolchain,
};
use cargo_test_support::{
    Execs, execs,
    paths::{self, CargoPathExt},
};

pub mod prelude {
    #[allow(unused_imports)]
    pub use cargo_test_support::prelude::*;
}

#[derive(Debug)]
pub struct GraphWrapper {
    pub graph: CrateGraph,
}

impl GraphWrapper {
    pub fn get(&self, c: &Crate) -> &PackageNode {
        let pkg = self.graph.inner.get(&c.package_id).unwrap();
        assert_eq!(c.name, pkg.name);
        assert_eq!(c.manifest_path(), pkg.manifest_path.as_std_path());
        assert!(pkg.is_workspace_member);

        pkg
    }

    pub fn get_pkg(&self, package_id: &PackageId) -> &PackageNode {
        self.graph.inner.get(package_id).unwrap_or_else(|| {
            panic!(
                "missing {package_id:?}; have {:?}",
                self.graph.inner.keys().collect::<Vec<_>>()
            )
        })
    }

    pub fn len(&self) -> usize {
        self.graph.inner.len()
    }

    pub fn contains(&self, package_id: &PackageId) -> bool {
        self.graph.inner.contains_key(package_id)
    }

    /// The only package with the given name
    #[track_caller]
    pub fn package(&self, name: &str) -> &PackageNode {
        let mut matches = self.graph.inner.values().filter(|pkg| pkg.name == name);

        let found = matches
            .next()
            .unwrap_or_else(|| panic!("no package named `{name}` in the graph"));
        assert!(
            matches.next().is_none(),
            "multiple packages named `{name}` in the graph"
        );
        found
    }

    /// Asserts that the packages in the graph are the ones with the given names, in any order
    #[track_caller]
    pub fn assert_packages<const N: usize>(&self, names: [&str; N]) {
        let mut actual: Vec<_> = self
            .graph
            .inner
            .values()
            .map(|pkg| pkg.name.as_str())
            .collect();
        actual.sort();
        let mut expected = names.to_vec();
        expected.sort();

        assert_eq!(actual, expected, "unexpected packages in the graph");
    }

    /// Asserts that the dependencies of the package with the given name are the ones with the given
    /// names, in any order, and that all of them are in the graph
    #[track_caller]
    pub fn assert_deps<const N: usize>(&self, name: &str, deps: [&str; N]) {
        let pkg = self.package(name);
        for dep in &pkg.dependencies {
            assert!(
                self.contains(&dep.id),
                "{name} depends on `{}`, which is not in the graph",
                dep.name
            );
        }

        let mut actual: Vec<_> = pkg
            .dependencies
            .iter()
            .map(|dep| dep.name.as_str())
            .collect();
        actual.sort();
        let mut expected = deps.to_vec();
        expected.sort();

        assert_eq!(actual, expected, "unexpected dependencies of {name}");
    }

    /// Lowers the graph into the crates that would be reported to rust-analyzer
    pub fn into_crates(self) -> Vec<cargo_subspace::rust_project::Crate> {
        self.graph.into_crates().unwrap()
    }
}

/// Runs the `cargo-subspace` binary against a crate or workspace
pub trait CargoSubspaceExt {
    fn check<P>(&self, path: P) -> Execs
    where
        P: AsRef<Path>;

    fn clippy<P>(&self, path: P) -> Execs
    where
        P: AsRef<Path>;

    fn discover_src<P>(&self, path: P) -> Execs
    where
        P: AsRef<Path>;

    fn discover_manifest(&self, path: &Path) -> Execs;

    /// A `cargo-subspace` command without any arguments, for tests that need full control over
    /// them (like flags that go before the subcommand)
    fn subspace(&self) -> Execs;
}

/// Builds a command that runs the `cargo-subspace` binary from the given directory
fn process(root: &Path) -> Execs {
    let program = snapbox::cmd::cargo_bin!("cargo-subspace").to_path_buf();
    let mut p = cargo_test_support::process(program);
    p.cwd(root);

    let mut execs = execs().with_process_builder(p);

    // cargo-test-support points `CARGO_HOME` at an empty per-test sandbox, and `cargo-subspace`
    // runs `$CARGO_HOME/bin/cargo` when it is set. Unset it so that `cargo-subspace` falls back to
    // the `cargo` and `rustc` on `PATH`, which is also what the in-process tests use.
    execs.env_remove("CARGO_HOME");

    execs
}

fn check_command(root: &Path, subcommand: &str, path: &Path) -> Execs {
    let mut execs = process(root);
    execs.arg(subcommand).arg(path);
    execs
}

fn discover_src_command(root: &Path, path: &Path) -> Execs {
    let mut execs = process(root);
    execs
        .arg("discover")
        // TODO(windows): the path is written into the JSON as it is displayed, and the backslashes
        // in a Windows path are escape chars in JSON
        .arg(format!("{{\"path\":\"{}\"}}", path.display()));
    execs
}

fn discover_manifest_command(root: &Path, path: &Path) -> Execs {
    let mut execs = process(root);
    execs
        .arg("discover")
        // TODO(windows): the path is written into the JSON as it is displayed, and the backslashes
        // in a Windows path are escape chars in JSON
        .arg(format!("{{\"buildfile\":\"{}\"}}", path.display()));
    execs
}

macro_rules! impl_cargo_subspace_ext {
    ($ty:ty) => {
        impl CargoSubspaceExt for $ty {
            fn check<P>(&self, path: P) -> Execs
            where
                P: AsRef<Path>,
            {
                check_command(&self.root, "check", path.as_ref())
            }

            fn clippy<P>(&self, path: P) -> Execs
            where
                P: AsRef<Path>,
            {
                check_command(&self.root, "clippy", path.as_ref())
            }

            fn discover_src<P>(&self, path: P) -> Execs
            where
                P: AsRef<Path>,
            {
                discover_src_command(&self.root, path.as_ref())
            }

            fn discover_manifest(&self, path: &Path) -> Execs {
                discover_manifest_command(&self.root, path)
            }

            fn subspace(&self) -> Execs {
                process(&self.root)
            }
        }
    };
}

impl_cargo_subspace_ext!(Crate);
impl_cargo_subspace_ext!(Workspace);

/// The parsed output of a `cargo-subspace discover` run: the project that was reported to
/// rust-analyzer.
///
/// Running a command through [`DiscoverOutput::run`] also checks that the output satisfies the
/// invariants that rust-analyzer relies on when it loads a `rust-project.json`, so every test that
/// goes through it doubles as a test of the output format. See [`DiscoverOutput::validate`].
pub struct DiscoverOutput {
    /// The `finished` message, exactly as it appeared in stdout
    raw: String,
    message: serde_json::Value,
    crates: Vec<serde_json::Value>,
    /// The text of the `progress` messages that were printed before the `finished` message
    progress: Vec<String>,
}

impl DiscoverOutput {
    /// Runs the command, which must succeed, and parses and validates the project in its stdout
    #[track_caller]
    pub fn run(execs: &mut Execs) -> Self {
        let output = execs.run();
        let parsed = Self::parse(&output.stdout);
        parsed.validate();
        parsed
    }

    /// Parses the `finished` message out of the stdout of a `discover` run, without validating it
    #[track_caller]
    pub fn parse(stdout: &[u8]) -> Self {
        let stdout = String::from_utf8(stdout.to_vec()).unwrap();

        let messages: Vec<_> = stdout
            .lines()
            .map(|line| {
                let message = serde_json::from_str::<serde_json::Value>(line)
                    .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line}"));
                (line, message)
            })
            .collect();

        // Progress messages are printed alongside the final result
        let progress = messages
            .iter()
            .filter(|(_, message)| message["kind"] == "progress")
            .map(|(_, message)| message["message"].as_str().unwrap().to_string())
            .collect();

        let mut finished = messages
            .into_iter()
            .filter(|(_, message)| message["kind"] == "finished");

        let (raw, message) = finished.next().expect("no `finished` message in stdout");
        assert!(finished.next().is_none(), "multiple `finished` messages");

        let mut parsed = Self::from_message(raw.to_string(), message);
        parsed.progress = progress;
        parsed
    }

    /// Wraps a `finished` message, without validating it
    pub fn from_message(raw: String, message: serde_json::Value) -> Self {
        let crates = message["project"]["crates"]
            .as_array()
            .expect("`project.crates` is not an array")
            .clone();
        Self {
            raw,
            message,
            crates,
            progress: vec![],
        }
    }

    /// The text of the progress messages that were printed before the project, which is what
    /// rust-analyzer shows to the user while discovery is running
    pub fn progress_messages(&self) -> &[String] {
        &self.progress
    }

    /// The `finished` message as it was printed, for comparing runs byte for byte
    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn buildfile(&self) -> &str {
        self.message["buildfile"].as_str().unwrap()
    }

    pub fn project(&self) -> &serde_json::Value {
        &self.message["project"]
    }

    pub fn len(&self) -> usize {
        self.crates.len()
    }

    pub fn crates(&self) -> &[serde_json::Value] {
        &self.crates
    }

    pub fn contains(&self, display_name: &str) -> bool {
        self.crates
            .iter()
            .any(|c| c["display_name"] == display_name)
    }

    /// Finds the only crate with the given display name, along with its index
    #[track_caller]
    pub fn find(&self, display_name: &str) -> (usize, &serde_json::Value) {
        let mut matches = self
            .crates
            .iter()
            .enumerate()
            .filter(|(_, c)| c["display_name"] == display_name);

        let found = matches
            .next()
            .unwrap_or_else(|| panic!("no crate named `{display_name}`"));
        assert!(
            matches.next().is_none(),
            "multiple crates named `{display_name}`"
        );
        found
    }

    /// Finds the only crate with the given display name and a root module that ends with the given
    /// path, for packages that have multiple targets
    #[track_caller]
    pub fn find_with_root_module(
        &self,
        display_name: &str,
        root_module: &str,
    ) -> (usize, &serde_json::Value) {
        let mut matches = self.crates.iter().enumerate().filter(|(_, c)| {
            c["display_name"] == display_name
                && Path::new(c["root_module"].as_str().unwrap()).ends_with(root_module)
        });

        let found = matches
            .next()
            .unwrap_or_else(|| panic!("no crate `{display_name}` with root module {root_module}"));
        assert!(
            matches.next().is_none(),
            "multiple crates `{display_name}` with root module {root_module}"
        );
        found
    }

    /// The (crate index, name) pairs of a crate's dependencies, sorted
    pub fn deps(&self, c: &serde_json::Value) -> Vec<(usize, String)> {
        let mut deps: Vec<_> = c["deps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|dep| {
                (
                    dep["crate"].as_u64().unwrap() as usize,
                    dep["name"].as_str().unwrap().to_string(),
                )
            })
            .collect();
        deps.sort();
        deps
    }

    /// Asserts that the packages in the project are the ones with the given names, in any order.
    ///
    /// The names are the ones that are reported, where hyphens are underscores. A package with
    /// several targets is reported as several crates, but counts once.
    #[track_caller]
    pub fn assert_packages<const N: usize>(&self, names: [&str; N]) {
        let mut actual: Vec<_> = self
            .crates
            .iter()
            .map(|c| c["display_name"].as_str().unwrap())
            .collect();
        actual.sort();
        actual.dedup();
        let mut expected = names.to_vec();
        expected.sort();

        assert_eq!(actual, expected, "unexpected packages in the project");
    }

    /// Asserts that the dependencies of the only crate with the given display name are imported by
    /// the given names, in any order
    #[track_caller]
    pub fn assert_deps<const N: usize>(&self, display_name: &str, deps: [&str; N]) {
        let (_, c) = self.find(display_name);
        let mut expected = deps.to_vec();
        expected.sort();

        assert_eq!(
            self.dep_names(c),
            expected,
            "unexpected dependencies of {display_name}"
        );
    }

    /// The names that a crate's dependencies are imported by, sorted
    pub fn dep_names(&self, c: &serde_json::Value) -> Vec<String> {
        let mut names: Vec<_> = self.deps(c).into_iter().map(|(_, name)| name).collect();
        names.sort();
        names
    }

    /// Checks the invariants that rust-analyzer relies on when it loads a `rust-project.json`, as
    /// described in <https://rust-analyzer.github.io/book/non_cargo_based_projects.html>, panicking
    /// if any of them are violated:
    ///
    /// - the paths that rust-analyzer reads are absolute and exist
    /// - editions, target kinds, and cfgs are in a format that rust-analyzer can parse
    /// - dependencies refer to crates in the `crates` array, and are not repeated by name
    /// - the crate graph has no cycles. rust-analyzer silently drops the dependency that would
    ///   close a cycle, and which one that is depends on the order of the crates.
    #[track_caller]
    pub fn validate(&self) {
        assert_eq!(self.message["kind"], "finished", "not a `finished` message");
        assert_absolute(&self.message["buildfile"], "buildfile");

        let project = self.project();
        let sysroot = assert_absolute(&project["sysroot"], "sysroot");
        assert!(sysroot.is_dir(), "sysroot does not exist: {sysroot:?}");
        if !project["sysroot_src"].is_null() {
            assert_absolute(&project["sysroot_src"], "sysroot_src");
        }

        assert!(!self.crates.is_empty(), "the project has no crates");

        for (index, c) in self.crates.iter().enumerate() {
            let name = c["display_name"].as_str().unwrap_or("<unnamed>");
            let context = format!("crate #{index} `{name}`");

            let root_module =
                assert_absolute(&c["root_module"], &format!("{context}: root_module"));
            assert!(
                root_module.is_file(),
                "{context}: root module does not exist: {root_module:?}"
            );

            let edition = c["edition"].as_str();
            assert!(
                matches!(edition, Some("2015" | "2018" | "2021" | "2024")),
                "{context}: invalid edition: {}",
                c["edition"]
            );

            assert!(
                c["is_proc_macro"].is_boolean(),
                "{context}: is_proc_macro is not a bool"
            );

            let mut dep_names = HashSet::new();
            for dep in c["deps"]
                .as_array()
                .unwrap_or_else(|| panic!("{context}: deps is not an array"))
            {
                let target = dep["crate"]
                    .as_u64()
                    .unwrap_or_else(|| panic!("{context}: dependency without a crate index"))
                    as usize;
                assert!(
                    target < self.crates.len(),
                    "{context}: dependency on crate #{target}, but there are only {} crates",
                    self.crates.len()
                );
                assert_ne!(target, index, "{context}: depends on itself");

                let dep_name = dep["name"].as_str().unwrap_or_default();
                assert!(
                    is_identifier(dep_name),
                    "{context}: `{dep_name}` is not a valid crate name"
                );
                assert!(
                    dep_names.insert(dep_name),
                    "{context}: more than one dependency named `{dep_name}`; rust-analyzer only \
                     keeps the last one"
                );
            }

            if let Some(build) = c.get("build").filter(|build| !build.is_null()) {
                assert!(
                    matches!(build["target_kind"].as_str(), Some("bin" | "lib" | "test")),
                    "{context}: invalid target_kind: {}",
                    build["target_kind"]
                );
                assert!(
                    !build["label"].as_str().unwrap_or_default().is_empty(),
                    "{context}: empty build label"
                );
                let build_file =
                    assert_absolute(&build["build_file"], &format!("{context}: build_file"));
                assert!(
                    build_file.is_file(),
                    "{context}: build file does not exist: {build_file:?}"
                );
            }

            for cfg in c
                .get("cfg")
                .and_then(|cfg| cfg.as_array())
                .into_iter()
                .flatten()
            {
                let cfg = cfg.as_str().unwrap_or_default();
                assert!(is_cfg(cfg), "{context}: `{cfg}` is not a valid cfg");
            }

            for (key, value) in c
                .get("env")
                .and_then(|env| env.as_object())
                .into_iter()
                .flatten()
            {
                assert!(
                    !key.is_empty(),
                    "{context}: empty environment variable name"
                );
                assert!(value.is_string(), "{context}: {key} is not a string");
            }

            if let Some(source) = c.get("source").filter(|source| !source.is_null()) {
                for dir in source["include_dirs"].as_array().into_iter().flatten() {
                    let dir = assert_absolute(dir, &format!("{context}: include_dirs"));
                    assert!(
                        dir.is_dir(),
                        "{context}: include dir does not exist: {dir:?}"
                    );
                }
            }

            if let Some(dylib) = c.get("proc_macro_dylib_path").filter(|d| !d.is_null()) {
                let dylib = assert_absolute(dylib, &format!("{context}: proc_macro_dylib_path"));
                assert!(
                    dylib.is_file(),
                    "{context}: dylib does not exist: {dylib:?}"
                );
            }
        }

        self.assert_acyclic();
    }

    #[track_caller]
    fn assert_acyclic(&self) {
        #[derive(Clone, Copy, PartialEq)]
        enum State {
            Unvisited,
            InProgress,
            Done,
        }

        let describe = |index: usize| {
            let c = &self.crates[index];
            format!(
                "{} ({})",
                c["display_name"].as_str().unwrap_or("<unnamed>"),
                c["root_module"].as_str().unwrap_or_default()
            )
        };

        let mut state = vec![State::Unvisited; self.crates.len()];

        for start in 0..self.crates.len() {
            if state[start] != State::Unvisited {
                continue;
            }

            // Depth-first search, where each stack entry is a crate and the next of its
            // dependencies to look at
            let mut stack = vec![(start, 0)];
            state[start] = State::InProgress;

            while let Some((index, next_dep)) = stack.pop() {
                let deps = self.crates[index]["deps"].as_array().unwrap();
                if let Some(dep) = deps.get(next_dep) {
                    stack.push((index, next_dep + 1));

                    let target = dep["crate"].as_u64().unwrap() as usize;
                    match state[target] {
                        State::Unvisited => {
                            state[target] = State::InProgress;
                            stack.push((target, 0));
                        }
                        State::InProgress => {
                            let mut cycle: Vec<_> = stack
                                .iter()
                                .map(|(index, _)| *index)
                                .skip_while(|index| *index != target)
                                .collect();
                            cycle.dedup();
                            cycle.push(target);
                            panic!(
                                "the crate graph has a dependency cycle: {}",
                                cycle
                                    .into_iter()
                                    .map(describe)
                                    .collect::<Vec<_>>()
                                    .join(" -> ")
                            );
                        }
                        State::Done => {}
                    }
                } else {
                    state[index] = State::Done;
                }
            }
        }
    }
}

/// Asserts that the value is an absolute path, and returns it
#[track_caller]
fn assert_absolute(value: &serde_json::Value, what: &str) -> PathBuf {
    let path = value
        .as_str()
        .unwrap_or_else(|| panic!("{what} is not a string: {value}"));
    assert!(
        Path::new(path).is_absolute(),
        "{what} is not an absolute path: {path}"
    );
    PathBuf::from(path)
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

/// Whether the string is a cfg that rust-analyzer can parse: either an atom (`unix`), or a key
/// with a quoted value (`feature="foo"`)
fn is_cfg(cfg: &str) -> bool {
    match cfg.split_once('=') {
        None => is_identifier(cfg),
        Some((key, value)) => {
            is_identifier(key) && value.len() >= 2 && value.starts_with('"') && value.ends_with('"')
        }
    }
}

/// Asserts that a proc macro dylib was built into the given target directory.
///
/// Cargo is changing the layout of the directories that it builds into: artifacts are in
/// `debug/deps` with the layout that it has used so far, and in `debug/build/<package>/<hash>/out`
/// with the new one. The file name also contains a hash that varies with the toolchain, target
/// directory, and build flags. So only the parts of the path that are stable are checked.
#[track_caller]
pub fn assert_proc_macro_dylib_path(dylib: &Path, target_dir: &Path, crate_name: &str) {
    let debug_dir = target_dir.join("debug");
    assert!(
        dylib.starts_with(&debug_dir),
        "dylib is not in {}: {}",
        debug_dir.display(),
        dylib.display()
    );
    assert!(dylib.is_file(), "dylib does not exist: {}", dylib.display());

    let file_name = dylib.file_name().unwrap().to_str().unwrap();
    assert!(
        file_name.starts_with(&format!("{}{crate_name}-", std::env::consts::DLL_PREFIX)),
        "unexpected dylib file name: {file_name}"
    );
    assert_eq!(dylib.extension().unwrap(), std::env::consts::DLL_EXTENSION);
}

/// Asserts that the `OUT_DIR` of a build script was created in the given target directory, in
/// `debug/build/<package>-<hash>/out` or, with the new layout of cargo's build directory,
/// `debug/build/<package>/<hash>/out`.
#[track_caller]
pub fn assert_build_script_out_dir_path(out_dir: &Path, target_dir: &Path, crate_name: &str) {
    assert_eq!(out_dir.file_name().unwrap(), "out");
    assert!(out_dir.is_dir(), "OUT_DIR was not created");

    let build_dir = target_dir.join("debug/build");
    let relative = out_dir.strip_prefix(&build_dir).unwrap_or_else(|_| {
        panic!(
            "OUT_DIR is not in {}: {}",
            build_dir.display(),
            out_dir.display()
        )
    });

    let package_dir = relative
        .components()
        .next()
        .unwrap()
        .as_os_str()
        .to_str()
        .unwrap();
    assert!(
        package_dir == crate_name || package_dir.starts_with(&format!("{crate_name}-")),
        "unexpected build script directory: {}",
        relative.display()
    );
}

/// The cfgs reported for a crate, sorted
pub fn cfg(c: &serde_json::Value) -> Vec<String> {
    let mut cfg: Vec<_> = c["cfg"]
        .as_array()
        .map(|cfg| {
            cfg.iter()
                .map(|c| c.as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default();
    cfg.sort();
    cfg
}

/// Runs `cargo-subspace discover` with the manifest at the given path as the build file, and
/// returns its validated output
#[track_caller]
pub fn discover_json(runner: &impl CargoSubspaceExt, manifest: PathBuf) -> DiscoverOutput {
    DiscoverOutput::run(&mut runner.discover_manifest(&manifest))
}

/// Like [`discover_json`], but with an extra flag passed to `cargo-subspace discover`
#[track_caller]
pub fn discover_json_with_flag(
    runner: &impl CargoSubspaceExt,
    manifest: PathBuf,
    flag: &str,
) -> DiscoverOutput {
    DiscoverOutput::run(runner.discover_manifest(&manifest).arg(flag))
}

pub trait DiscoverRunnerExt {
    fn build(self) -> GraphWrapper;
}

impl DiscoverRunnerExt for DiscoverRunner {
    fn build(self) -> GraphWrapper {
        let graph = self.run().unwrap();
        GraphWrapper { graph }
    }
}

pub fn discover<T>(manifest_path: T) -> GraphWrapper
where
    T: Into<PathBuf>,
{
    discover_path_builder(manifest_path).build()
}

pub fn discover_path_builder<T>(manifest_path: T) -> DiscoverRunner
where
    T: Into<PathBuf>,
{
    let p: PathBuf = manifest_path.into();
    let toolchain = Toolchain::new(None);

    DiscoverRunner::new(toolchain, p.try_into().unwrap())
}

pub fn lib<T>(name: T) -> CrateBuilder
where
    T: Into<String>,
{
    CrateBuilder::lib(name)
}

pub fn bin<T>(name: T) -> CrateBuilder
where
    T: Into<String>,
{
    CrateBuilder::bin(name)
}

pub enum Dependency {
    Path(PathBuf),
    /// The value of the dependency, as it appears in `Cargo.toml` (e.g. `{ path = "../a" }`)
    Raw(String),
    External {
        version: String,
        no_default_features: bool,
    },
}

/// Whether the cargo that the tests use can build crates of the 2024 edition, which it can since
/// version 1.85.
///
/// The crates that the tests generate are of the 2024 edition when it can, because that's the one
/// that new projects use. Older versions of cargo are tested with the 2021 edition.
pub fn supports_edition_2024() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();

    *SUPPORTED.get_or_init(|| {
        Toolchain::new(None)
            .cargo_version()
            .is_some_and(|version| version >= Version::new(1, 85, 0))
    })
}

fn default_edition() -> &'static str {
    if supports_edition_2024() {
        "2024"
    } else {
        "2021"
    }
}

/// The ids that cargo gives to the packages in the workspace with the given manifest, by name.
///
/// The format of the ids depends on the version of cargo, so they are asked for instead of being
/// built.
fn package_ids(manifest: &Path) -> HashMap<String, PackageId> {
    cargo_metadata::MetadataCommand::new()
        .manifest_path(manifest)
        .no_deps()
        .exec()
        .unwrap_or_else(|e| panic!("could not get the metadata of {}: {e}", manifest.display()))
        .packages
        .into_iter()
        .map(|package| (package.name.to_string(), package.id))
        .collect()
}

pub struct Crate {
    pub name: String,
    pub root: PathBuf,
    pub package_id: PackageId,
}

impl Crate {
    pub fn file<T>(&self, path: T) -> PathBuf
    where
        T: AsRef<Path>,
    {
        self.root.join(path.as_ref())
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("Cargo.toml")
    }

    /// Runs discover in-process on this crate
    pub fn graph(&self) -> GraphWrapper {
        discover(self.manifest_path())
    }

    /// A discover runner for this crate, to configure before it runs
    pub fn runner(&self) -> DiscoverRunner {
        discover_path_builder(self.manifest_path())
    }

    /// Runs `cargo-subspace discover` on this crate
    #[track_caller]
    pub fn discover(&self) -> DiscoverOutput {
        discover_json(self, self.manifest_path())
    }

    /// Like [`Crate::discover`], but with an extra flag passed to `cargo-subspace discover`
    #[track_caller]
    pub fn discover_with_flag(&self, flag: &str) -> DiscoverOutput {
        discover_json_with_flag(self, self.manifest_path(), flag)
    }

    pub fn as_dependency(&self) -> cargo_subspace::graph::Dependency {
        cargo_subspace::graph::Dependency {
            id: self.package_id.clone(),
            name: self.name.clone(),
        }
    }
}

pub struct CrateBuilder {
    name: String,
    dependencies: HashMap<String, Dependency>,
    features: HashMap<String, Vec<String>>,
    files: HashMap<PathBuf, String>,
    proc_macro: bool,
    edition: String,
    /// Extra tables to append to `Cargo.toml` as-is
    manifest: String,
}

impl CrateBuilder {
    pub fn lib<T>(name: T) -> Self
    where
        T: Into<String>,
    {
        let mut files = HashMap::new();
        files.insert("src/lib.rs".into(), "".into());

        Self {
            name: name.into(),
            files,
            features: HashMap::new(),
            dependencies: HashMap::new(),
            proc_macro: false,
            edition: default_edition().into(),
            manifest: String::new(),
        }
    }

    pub fn bin<T>(name: T) -> Self
    where
        T: Into<String>,
    {
        let mut files = HashMap::new();
        files.insert("src/main.rs".into(), "fn main() {}".into());

        Self {
            name: name.into(),
            files,
            features: HashMap::new(),
            dependencies: HashMap::new(),
            proc_macro: false,
            edition: default_edition().into(),
            manifest: String::new(),
        }
    }

    /// Adds a dependency on a package from a registry, at exactly the given version
    pub fn dependency<T, S>(&mut self, name: T, version: S, no_default_features: bool) -> &mut Self
    where
        T: Into<String>,
        S: Into<String>,
    {
        self.dependencies.insert(
            name.into(),
            Dependency::External {
                version: version.into(),
                no_default_features,
            },
        );
        self
    }

    /// Adds a dependency whose value is written to `Cargo.toml` as-is, for things like renamed
    /// dependencies (`{ path = "../a", package = "a" }`) and optional dependencies
    pub fn raw_dependency<T, S>(mut self, name: T, value: S) -> Self
    where
        T: Into<String>,
        S: Into<String>,
    {
        self.dependencies
            .insert(name.into(), Dependency::Raw(value.into()));
        self
    }

    /// Sets the edition of this crate (the default is 2024)
    pub fn edition<T>(mut self, edition: T) -> Self
    where
        T: Into<String>,
    {
        self.edition = edition.into();
        self
    }

    /// Appends extra tables to this crate's `Cargo.toml` as-is, like `[dev-dependencies]`,
    /// `[build-dependencies]`, `[lib]`, or `[workspace]`. This can't contain `[dependencies]`,
    /// `[features]`, or the `[package]` table; use the other builder methods for those.
    pub fn manifest<T>(mut self, tables: T) -> Self
    where
        T: AsRef<str>,
    {
        self.manifest.push('\n');
        self.manifest.push_str(tables.as_ref());
        self.manifest.push('\n');
        self
    }

    /// Makes this crate a proc macro crate
    pub fn proc_macro(mut self) -> Self {
        self.proc_macro = true;
        self
    }

    pub fn path_dependency(mut self, other: &Self) -> Self {
        let root = paths::root();
        self.dependencies.insert(
            other.name.clone(),
            Dependency::Path(root.join(other.name.clone())),
        );
        self
    }

    pub fn feature<T, S, I>(mut self, name: T, include: I) -> Self
    where
        T: Into<String>,
        S: Into<String>,
        I: IntoIterator<Item = S>,
    {
        self.features
            .insert(name.into(), include.into_iter().map(|i| i.into()).collect());
        self
    }

    // paths relative to crate root
    pub fn file<T, S>(mut self, path: T, contents: S) -> Self
    where
        T: Into<PathBuf>,
        S: Into<String>,
    {
        self.files.insert(path.into(), contents.into());
        self
    }

    pub fn build(self) -> Crate {
        let root = paths::root();
        // The root is already empty: `#[cargo_test]` clears it before each test starts. Clearing it
        // here would also delete anything that the test set up in it first, like a fake registry.
        root.mkdir_p();

        let name = self.name.clone();
        self.build_with_root(&root);

        let package_id = package_ids(&root.join("Cargo.toml"))
            .remove(&name)
            .unwrap_or_else(|| panic!("cargo doesn't know about the package {name}"));

        Crate {
            root,
            name,
            package_id,
        }
    }

    fn build_with_root(self, root: &Path) {
        Self::create_cargo_toml(
            root,
            &self.name,
            self.dependencies,
            self.features,
            self.proc_macro,
            &self.edition,
            &self.manifest,
        );

        for (path, contents) in self.files {
            let path = root.join(path);
            path.parent().unwrap().mkdir_p();

            fs::write(&path, &contents)
                .unwrap_or_else(|e| panic!("could not create file {}: {}", path.display(), e));
        }
    }

    fn create_cargo_toml(
        root: &Path,
        name: &str,
        dependencies: HashMap<String, Dependency>,
        features: HashMap<String, Vec<String>>,
        proc_macro: bool,
        edition: &str,
        manifest: &str,
    ) {
        let mut cargo_toml = format!(
            r#"
            [package]
            name = "{name}"
            version = "0.1.0"
            edition = "{edition}"

            [dependencies]
        "#,
        );

        for (name, dependency) in dependencies {
            let s = match dependency {
                // TODO(windows): the backslashes in a Windows path are escape chars in a TOML basic
                // string
                Dependency::Path(path) => format!(r#"{name} = {{ path = "{}"}}"#, path.display()),
                Dependency::Raw(value) => format!("{name} = {value}"),
                Dependency::External {
                    version,
                    no_default_features: false,
                } => format!(r#"{name} = "={version}""#),
                Dependency::External {
                    version,
                    no_default_features: true,
                } => format!(r#"{name} = {{ version = "={version}", default-features = false }}"#),
            };

            cargo_toml.push('\n');
            cargo_toml.push_str(&s);
        }

        if proc_macro {
            cargo_toml.push_str("\n[lib]\nproc-macro = true\n");
        }

        cargo_toml.push_str("\n[features]");

        for (name, includes) in features {
            let includes = includes
                .into_iter()
                .map(|i| format!(r#""{i}""#))
                .collect::<Vec<_>>()
                .join(", ");

            cargo_toml.push_str(&format!("\n{name} = [{includes}]"));
        }

        cargo_toml.push('\n');
        cargo_toml.push_str(manifest);

        let path = root.join("Cargo.toml");
        fs::write(&path, &cargo_toml)
            .unwrap_or_else(|e| panic!("could not create file {}: {}", path.display(), e));
    }
}

pub struct Workspace {
    root: PathBuf,
    members: HashMap<String, Crate>,
    excludes: HashMap<String, Crate>,
}

impl Workspace {
    pub fn file<T>(&self, path: T) -> PathBuf
    where
        T: AsRef<Path>,
    {
        self.root.join(path.as_ref())
    }

    pub fn member<T>(&self, name: T) -> &Crate
    where
        T: AsRef<str>,
    {
        self.members.get(name.as_ref()).unwrap()
    }

    /// The member or excluded crate with the given name
    #[track_caller]
    fn crate_named(&self, name: &str) -> &Crate {
        self.members
            .get(name)
            .or_else(|| self.excludes.get(name))
            .unwrap_or_else(|| panic!("no crate named `{name}` in the workspace"))
    }

    /// Runs discover in-process on the crate with the given name
    #[track_caller]
    pub fn graph(&self, name: &str) -> GraphWrapper {
        self.crate_named(name).graph()
    }

    /// A discover runner for the crate with the given name, to configure before it runs
    #[track_caller]
    pub fn runner(&self, name: &str) -> DiscoverRunner {
        self.crate_named(name).runner()
    }

    /// Runs `cargo-subspace discover` on the crate with the given name
    #[track_caller]
    pub fn discover(&self, name: &str) -> DiscoverOutput {
        discover_json(self, self.crate_named(name).manifest_path())
    }

    /// Like [`Workspace::discover`], but with an extra flag passed to `cargo-subspace discover`
    #[track_caller]
    pub fn discover_with_flag(&self, name: &str, flag: &str) -> DiscoverOutput {
        discover_json_with_flag(self, self.crate_named(name).manifest_path(), flag)
    }

    pub fn excluded<T>(&self, name: T) -> &Crate
    where
        T: AsRef<str>,
    {
        self.excludes.get(name.as_ref()).unwrap()
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("Cargo.toml")
    }
}

pub fn workspace() -> WorkspaceBuilder {
    WorkspaceBuilder::new()
}

/// Creates a workspace in which every one of the given crates is a member
pub fn workspace_of<I>(members: I) -> Workspace
where
    I: IntoIterator<Item = CrateBuilder>,
{
    members
        .into_iter()
        .fold(workspace(), |w, member| w.member(member))
        .build()
}

pub struct WorkspaceBuilder {
    members: HashMap<PathBuf, CrateBuilder>,
    exclude: HashMap<PathBuf, CrateBuilder>,
}

impl WorkspaceBuilder {
    pub fn new() -> Self {
        Self {
            members: HashMap::new(),
            exclude: HashMap::new(),
        }
    }

    pub fn member(mut self, member: CrateBuilder) -> Self {
        self.members.insert(member.name.clone().into(), member);
        self
    }

    pub fn member_with_path<T>(mut self, path: T, member: CrateBuilder) -> Self
    where
        T: Into<PathBuf>,
    {
        self.members.insert(path.into(), member);
        self
    }

    pub fn excluded(mut self, excluded: CrateBuilder) -> Self {
        self.exclude.insert(excluded.name.clone().into(), excluded);
        self
    }

    pub fn excluded_with_path<T>(mut self, path: T, exclude: CrateBuilder) -> Self
    where
        T: Into<PathBuf>,
    {
        self.exclude.insert(path.into(), exclude);
        self
    }

    pub fn build(self) -> Workspace {
        let root = paths::root();
        // The root is already empty: `#[cargo_test]` clears it before each test starts. Clearing it
        // here would also delete anything that the test set up in it first, like a fake registry.
        root.mkdir_p();

        self.create_cargo_toml();

        let mut member_roots = vec![];
        for (crate_root, member) in self.members {
            let abs = root.join(crate_root);
            abs.mkdir_p();

            let name = member.name.clone();
            member.build_with_root(&abs);
            member_roots.push((name, abs));
        }

        let mut exclude_roots = vec![];
        for (crate_root, exclude) in self.exclude {
            let abs = root.join(crate_root);
            abs.mkdir_p();

            let name = exclude.name.clone();
            exclude.build_with_root(&abs);
            exclude_roots.push((name, abs));
        }

        // The ids can only be asked for once all of the crates exist, because they can depend on
        // each other
        let mut ids = package_ids(&root.join("Cargo.toml"));
        let members = member_roots
            .into_iter()
            .map(|(name, abs)| {
                let package_id = ids
                    .remove(&name)
                    .unwrap_or_else(|| panic!("cargo doesn't know about the member {name}"));
                (
                    name.clone(),
                    Crate {
                        name,
                        root: abs,
                        package_id,
                    },
                )
            })
            .collect();

        let excludes = exclude_roots
            .into_iter()
            .map(|(name, abs)| {
                let package_id = package_ids(&abs.join("Cargo.toml"))
                    .remove(&name)
                    .unwrap_or_else(|| {
                        panic!("cargo doesn't know about the excluded crate {name}")
                    });
                (
                    name.clone(),
                    Crate {
                        name,
                        root: abs,
                        package_id,
                    },
                )
            })
            .collect();

        Workspace {
            root,
            members,
            excludes,
        }
    }

    fn create_cargo_toml(&self) {
        let members = self
            .members
            .keys()
            .map(|p| format!(r#""{}""#, p.display()))
            .collect::<Vec<_>>()
            .join(", ");
        let exclude = self
            .exclude
            .keys()
            .map(|p| format!(r#""{}""#, p.display()))
            .collect::<Vec<_>>()
            .join(", ");

        let resolver = if supports_edition_2024() { "3" } else { "2" };

        let contents = format!(
            r#"
            [workspace]
            resolver = "{resolver}"
            members = [{members}]
            exclude = [{exclude}]
        "#
        );
        let path = paths::root().join("Cargo.toml");
        fs::write(&path, &contents)
            .unwrap_or_else(|e| panic!("could not create file {}: {}", path.display(), e));
    }
}
