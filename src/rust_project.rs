use std::collections::HashMap;
use std::fmt::Display;

use cargo_metadata::Edition;
use cargo_metadata::camino::Utf8PathBuf;
use serde::Serialize;

use crate::util::FilePathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct ProjectJson {
    /// Path to the sysroot directory.
    ///
    /// The sysroot is where rustc looks for the
    /// crates that are built-in to rust, such as
    /// std.
    ///
    /// https://doc.rust-lang.org/rustc/command-line-arguments.html#--sysroot-override-the-system-root
    ///
    /// To see the current value of sysroot, you
    /// can query rustc:
    ///
    /// ```sh
    /// $ rustc --print sysroot
    /// /Users/yourname/.rustup/toolchains/stable-x86_64-apple-darwin
    /// ```
    pub sysroot: Utf8PathBuf,
    /// Path to the directory with *source code* of
    /// sysroot crates.
    ///
    /// By default, this is `lib/rustlib/src/rust/library`
    /// relative to the sysroot.
    ///
    /// It should point to the directory where std,
    /// core, and friends can be found:
    ///
    /// https://github.com/rust-lang/rust/tree/master/library.
    ///
    /// If provided, rust-analyzer automatically adds
    /// dependencies on sysroot crates. Conversely,
    /// if you omit this path, you can specify sysroot
    /// dependencies yourself and, for example, have
    /// several different "sysroots" in one graph of
    /// crates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sysroot_src: Option<Utf8PathBuf>,
    // /// A ProjectJson describing the crates of the sysroot.
    // #[serde(skip_serializing_if = "Option::is_none")]
    // sysroot_project: Option<Box<ProjectJson>>,
    // /// List of groups of common cfg values, to allow
    // /// sharing them between crates.
    // ///
    // /// Maps from group name to its cfgs. Cfg follow
    // /// the same format as `Crate.cfg`.
    // cfg_groups: HashMap<String, Vec<String>>,
    /// The set of crates comprising the current
    /// project. Must include all transitive
    /// dependencies as well as sysroot crate (libstd,
    /// libcore and such).
    pub crates: Vec<Crate>,
    /// Configuration for CLI commands.
    ///
    /// These are used for running and debugging binaries
    /// and tests without encoding build system-specific
    /// knowledge into rust-analyzer.
    ///
    /// # Example
    ///
    /// Below is an example of a test runnable. `{label}` and `{test_id}`
    /// are explained in `Runnable::args`'s documentation below.
    ///
    /// ```json
    /// {
    ///     "program": "buck",
    ///     "args": [
    ///         "test",
    ///          "{label}",
    ///          "--",
    ///          "{test_id}",
    ///          "--print-passing-details"
    ///     ],
    ///     "cwd": "/home/user/repo-root/",
    ///     "kind": "testOne"
    /// }
    /// ```
    pub runnables: Vec<Runnable>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Crate {
    /// Optional crate name used for display purposes,
    /// without affecting semantics. See the `deps`
    /// key for semantically-significant crate names.
    pub display_name: Option<String>,
    /// Path to the root module of the crate.
    pub root_module: FilePathBuf,
    /// Edition of the crate.
    pub edition: Edition,
    /// The version of the crate. Used for calculating
    /// the correct docs.rs URL.
    pub version: Option<String>,
    /// Dependencies
    pub deps: Vec<Dep>,
    /// Should this crate be treated as a member of
    /// current "workspace".
    ///
    /// By default, inferred from the `root_module`
    /// (members are the crates which reside inside
    /// the directory opened in the editor).
    ///
    /// Set this to `false` for things like standard
    /// library and 3rd party crates to enable
    /// performance optimizations (rust-analyzer
    /// assumes that non-member crates don't change).
    pub is_workspace_member: bool,
    /// Optionally specify the (super)set of `.rs`
    /// files comprising this crate.
    ///
    /// By default, rust-analyzer assumes that only
    /// files under `root_module.parent` can belong
    /// to a crate. `include_dirs` are included
    /// recursively, unless a subdirectory is in
    /// `exclude_dirs`.
    ///
    /// Different crates can share the same `source`.
    ///
    /// If two crates share an `.rs` file in common,
    /// they *must* have the same `source`.
    /// rust-analyzer assumes that files from one
    /// source can't refer to files in another source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<CrateSource>,
    // /// List of cfg groups this crate inherits.
    // ///
    // /// All cfg in these groups will be concatenated to
    // /// `cfg`. It is impossible to replace a value from
    // /// the groups.
    // cfg_groups: Option<Vec<String>>,
    /// The set of cfgs activated for a given crate, like
    /// `["unix", "feature=\"foo\"", "feature=\"bar\""]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cfg: Vec<String>,
    /// Target tuple for this Crate.
    ///
    /// Used when running `rustc --print cfg`
    /// to get target-specific cfgs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Environment variables, used for
    /// the `env!` macro
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,

    /// Whether the crate is a proc-macro crate.
    pub is_proc_macro: bool,
    /// For proc-macro crates, path to compiled
    /// proc-macro (.so file).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proc_macro_dylib_path: Option<FilePathBuf>,

    /// Repository, matching the URL that would be used
    /// in Cargo.toml.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,

    /// Build-specific data about this crate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildInfo>,

    #[serde(default)]
    pub proc_macro_cwd: Option<FilePathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Runnable {
    /// The program invoked by the runnable.
    ///
    /// For example, this might be `cargo`, `buck`, or `bazel`.
    pub program: String,
    /// The arguments passed to `program`.
    pub args: Vec<String>,
    /// The current working directory of the runnable.
    pub cwd: String,
    /// Used to decide what code lens to offer.
    ///
    /// `testOne`: This runnable will be used when the user clicks the 'Run Test'
    /// CodeLens above a test.
    ///
    /// The args for testOne can contain two template strings:
    /// `{label}` and `{test_id}`. `{label}` will be replaced
    /// with the `Build::label` and `{test_id}` will be replaced
    /// with the test name.
    pub kind: RunnableKind,
}

#[allow(unused)]
#[derive(Debug, Clone, Serialize)]
#[serde(into = "String")]
pub enum RunnableKind {
    TestOne,
    String(String),
}

impl Display for RunnableKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TestOne => write!(f, "testOne"),
            Self::String(s) => write!(f, "{s}"),
        }
    }
}

impl From<RunnableKind> for String {
    fn from(value: RunnableKind) -> Self {
        value.to_string()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Dep {
    /// Index of a crate in the `crates` array.
    #[serde(rename = "crate")]
    pub crate_index: usize,
    /// Name as should appear in the (implicit)
    /// `extern crate name` declaration.
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrateSource {
    pub include_dirs: Vec<String>,
    pub exclude_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildInfo {
    /// The name associated with this crate.
    ///
    /// This is determined by the build system that produced
    /// the `rust-project.json` in question. For instance, if buck were used,
    /// the label might be something like `//ide/rust/rust-analyzer:rust-analyzer`.
    ///
    /// Do not attempt to parse the contents of this string; it is a build system-specific
    /// identifier similar to `Crate::display_name`.
    pub label: String,
    /// Path corresponding to the build system-specific file defining the crate.
    pub build_file: String,
    /// The kind of target.
    ///
    /// This information is used to determine what sort
    /// of runnable codelens to provide, if any.
    pub target_kind: TargetKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TargetKind {
    Bin,
    Lib,
    Test,
}

impl TargetKind {
    pub fn new(kinds: &[cargo_metadata::TargetKind]) -> TargetKind {
        for kind in kinds {
            return match kind {
                cargo_metadata::TargetKind::Bin => TargetKind::Bin,
                cargo_metadata::TargetKind::Test => TargetKind::Test,
                cargo_metadata::TargetKind::Bench => TargetKind::Test,
                cargo_metadata::TargetKind::Example => TargetKind::Bin,
                cargo_metadata::TargetKind::CustomBuild => TargetKind::Bin,
                cargo_metadata::TargetKind::ProcMacro => TargetKind::Lib,
                cargo_metadata::TargetKind::Lib
                | cargo_metadata::TargetKind::DyLib
                | cargo_metadata::TargetKind::CDyLib
                | cargo_metadata::TargetKind::StaticLib
                | cargo_metadata::TargetKind::RLib => TargetKind::Lib,
                _ => continue,
            };
        }

        TargetKind::Bin
    }
}
