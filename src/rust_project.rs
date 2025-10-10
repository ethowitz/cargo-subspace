use std::collections::{HashMap, HashSet};
use std::fmt::{self, Display};
use std::fs::File;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use cargo_metadata::camino::Utf8PathBuf;
use cargo_metadata::semver::Version;
use cargo_metadata::{BuildScript, Edition, Metadata, PackageId};
use petgraph::dot::Dot;
use petgraph::graph::NodeIndex;
use petgraph::prelude::StableDiGraph;
use serde::Serialize;
use tracing::debug;

use crate::proc_macros::build_compile_time_dependencies;
use crate::util::{FilePath, FilePathBuf};
use crate::{Context, log_progress};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProjectJson {
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
    /// ```
    /// $ rustc --print sysroot
    /// /Users/yourname/.rustup/toolchains/stable-x86_64-apple-darwin
    /// ```
    sysroot: Utf8PathBuf,
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
    sysroot_src: Option<Utf8PathBuf>,
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
    crates: Vec<Crate>,
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
    runnables: Vec<Runnable>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Crate {
    /// Optional crate name used for display purposes,
    /// without affecting semantics. See the `deps`
    /// key for semantically-significant crate names.
    display_name: Option<String>,
    /// Path to the root module of the crate.
    root_module: FilePathBuf,
    /// Edition of the crate.
    edition: Edition,
    /// The version of the crate. Used for calculating
    /// the correct docs.rs URL.
    version: Option<String>,
    /// Dependencies
    deps: Vec<Dep>,
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
    is_workspace_member: bool,
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
    source: Option<CrateSource>,
    // /// List of cfg groups this crate inherits.
    // ///
    // /// All cfg in these groups will be concatenated to
    // /// `cfg`. It is impossible to replace a value from
    // /// the groups.
    // cfg_groups: Option<Vec<String>>,
    /// The set of cfgs activated for a given crate, like
    /// `["unix", "feature=\"foo\"", "feature=\"bar\""]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    // TODO: are any/all etc. supported in these cfgs? Answer: no
    // TODO: get target from metadata and set as cfg and then test
    cfg: Vec<String>,
    /// Target tuple for this Crate.
    ///
    /// Used when running `rustc --print cfg`
    /// to get target-specific cfgs.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// Environment variables, used for
    /// the `env!` macro
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    env: HashMap<String, String>,

    /// Whether the crate is a proc-macro crate.
    is_proc_macro: bool,
    /// For proc-macro crates, path to compiled
    /// proc-macro (.so file).
    #[serde(skip_serializing_if = "Option::is_none")]
    proc_macro_dylib_path: Option<FilePathBuf>,

    /// Repository, matching the URL that would be used
    /// in Cargo.toml.
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<String>,

    /// Build-specific data about this crate.
    #[serde(skip_serializing_if = "Option::is_none")]
    build: Option<BuildInfo>,

    #[serde(default)]
    proc_macro_cwd: Option<FilePathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Runnable {
    /// The program invoked by the runnable.
    ///
    /// For example, this might be `cargo`, `buck`, or `bazel`.
    program: String,
    /// The arguments passed to `program`.
    args: Vec<String>,
    /// The current working directory of the runnable.
    cwd: String,
    /// Used to decide what code lens to offer.
    ///
    /// `testOne`: This runnable will be used when the user clicks the 'Run Test'
    /// CodeLens above a test.
    ///
    /// The args for testOne can contain two template strings:
    /// `{label}` and `{test_id}`. `{label}` will be replaced
    /// with the `Build::label` and `{test_id}` will be replaced
    /// with the test name.
    kind: RunnableKind,
}

#[allow(unused)]
#[derive(Debug, Clone, Serialize)]
#[serde(into = "String")]
pub(crate) enum RunnableKind {
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
pub(crate) struct Dep {
    /// Index of a crate in the `crates` array.
    #[serde(rename = "crate")]
    crate_index: usize,
    /// Name as should appear in the (implicit)
    /// `extern crate name` declaration.
    name: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CrateSource {
    include_dirs: Vec<String>,
    exclude_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct BuildInfo {
    /// The name associated with this crate.
    ///
    /// This is determined by the build system that produced
    /// the `rust-project.json` in question. For instance, if buck were used,
    /// the label might be something like `//ide/rust/rust-analyzer:rust-analyzer`.
    ///
    /// Do not attempt to parse the contents of this string; it is a build system-specific
    /// identifier similar to `Crate::display_name`.
    label: String,
    /// Path corresponding to the build system-specific file defining the crate.
    build_file: String,
    /// The kind of target.
    ///
    /// This information is used to determine what sort
    /// of runnable codelens to provide, if any.
    target_kind: TargetKind,
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

pub(crate) fn find_sysroot(ctx: &Context) -> Result<Utf8PathBuf> {
    let p: PathBuf = String::from_utf8(ctx.rustc().arg("--print").arg("sysroot").output()?.stdout)?
        .trim()
        .into();

    Utf8PathBuf::from_path_buf(p).map_err(|_| anyhow!("Path contains non-UTF-8 characters"))
}

pub(crate) fn graphviz(metadata: Metadata, manifest_path: FilePath<'_>) -> Result<String> {
    let mut graph = PackageGraph::lower_from_metadata(metadata)?;

    log_progress("Pruning metadata")?;
    graph.prune(manifest_path)?;

    let g = graph.into_petgraph();
    Ok(format!("{:?}", Dot::new(&g)))
}

pub(crate) fn compute_project_json(
    ctx: &Context,
    metadata: Metadata,
    manifest_path: FilePath<'_>,
) -> Result<ProjectJson> {
    log_progress("Finding sysroot")?;
    let sysroot = find_sysroot(ctx)?;
    debug!(sysroot = %sysroot);

    let sysroot_src = sysroot.join("lib/rustlib/src/rust/library");
    let crates = crates_from_metadata(ctx, metadata, manifest_path)?;

    Ok(ProjectJson {
        sysroot,
        sysroot_src: Some(sysroot_src),
        // TODO: do i need this? buck excludes it...
        // sysroot_project: None,
        // TODO: do i need this? buck excludes it...
        // cfg_groups: HashMap::new(),
        crates,
        // TODO: Add support for runnables
        runnables: vec![],
    })
}

/// Represents one target of a single package
#[derive(Clone)]
pub(crate) struct PackageNode {
    pub(crate) name: String,
    targets: Vec<Target>,
    manifest_path: FilePathBuf,
    version: Version,
    is_workspace_member: bool,
    repository: Option<String>,
    features: Vec<String>,
    // other_cfgs: Vec<String>,
    dependencies: Vec<Dependency>,
}

#[derive(Clone)]
pub(crate) struct Dependency {
    id: PackageId,
    name: String,
}

#[derive(Clone)]
pub(crate) struct Target {
    name: String,
    edition: Edition,
    kind: Vec<cargo_metadata::TargetKind>,
    root_module: FilePathBuf,
}

impl Target {
    fn is_proc_macro(&self) -> bool {
        self.kind
            .iter()
            .any(|k| matches!(k, cargo_metadata::TargetKind::ProcMacro))
    }
}

struct PackageGraph {
    graph: HashMap<PackageId, PackageNode>,
}

struct Vertex {
    name: String,
    is_workspace_member: bool,
    version: Version,
}

impl fmt::Debug for Vertex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)?;

        if self.is_workspace_member {
            write!(f, " (workspace)")?;
        }

        Ok(())
    }
}

// fn graph_from_metadata(ctx: &Context, metadata: Metadata) -> StableDiGraph<PackageNode, NodeDep>

impl PackageGraph {
    fn into_petgraph(self) -> StableDiGraph<Vertex, ()> {
        let mut pkg_id_to_graph_index: HashMap<PackageId, NodeIndex<u32>> = HashMap::new();
        let mut g = StableDiGraph::with_capacity(
            self.graph.len(),
            self.graph
                .values()
                .map(|node| node.dependencies.len())
                .sum(),
        );

        for (id, pkg) in self.graph.iter() {
            let index = g.add_node(Vertex {
                name: pkg.name.clone(),
                is_workspace_member: pkg.is_workspace_member,
                version: pkg.version.clone(),
            });
            pkg_id_to_graph_index.insert(id.clone(), index);
        }

        for (id, pkg) in self.graph.into_iter() {
            let src = *pkg_id_to_graph_index.get(&id).unwrap();
            for dep in pkg.dependencies {
                let dest = *pkg_id_to_graph_index.get(&dep.id).unwrap();

                g.add_edge(src, dest, ());
            }
        }

        g
    }

    fn lower_from_metadata(metadata: Metadata) -> Result<Self> {
        let mut graph = HashMap::new();
        let workspace_members: HashSet<&PackageId> =
            HashSet::from_iter(metadata.workspace_members.iter());
        let mut features: HashMap<PackageId, HashSet<String>> = HashMap::new();
        let mut dependencies: HashMap<PackageId, Vec<Dependency>> = HashMap::new();

        if let Some(it) = metadata.resolve {
            for node in it.nodes {
                features
                    .entry(node.id.clone())
                    .or_default()
                    .extend(node.features.iter().map(|feat| feat.to_string()));

                // TODO: test that this works with renamed dependencies
                dependencies
                    .entry(node.id)
                    .or_default()
                    .extend(node.deps.into_iter().map(|dep| Dependency {
                        id: dep.pkg,
                        name: dep.name,
                    }));
            }
        }

        for mut package in metadata.packages {
            // If the package is not a member of the workspace, don't include any test, example, or
            // bench targets.
            if !workspace_members.contains(&package.id) {
                package
                    .targets
                    .retain(|t| !t.is_test() && !t.is_example() && !t.is_bench());
            }

            let targets = package
                .targets
                .into_iter()
                .map(|t| {
                    Ok(Target {
                        name: t.name,
                        edition: t.edition,
                        kind: t.kind,
                        root_module: t.src_path.try_into()?,
                    })
                })
                .collect::<Result<Vec<_>>>()?;

            let node = PackageNode {
                name: package.name.to_string(),
                targets,
                manifest_path: package.manifest_path.try_into()?,
                version: package.version,
                is_workspace_member: workspace_members.contains(&package.id),
                repository: package.repository,
                features: features
                    .get(&package.id)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
                dependencies: dependencies.get(&package.id).cloned().unwrap_or_default(),
            };

            graph.insert(package.id, node);
        }

        Ok(Self { graph })
    }

    /// Prunes the graph such that the remaining nodes consist only of:
    /// 1. The package with the given manifest path; and
    /// 2. The dependencies of that package
    fn prune(&mut self, manifest_path: FilePath<'_>) -> Result<()> {
        let abs = std::path::absolute(manifest_path.as_std_path())?;
        let Some((id, _)) = self
            .graph
            .iter()
            .find(|(_, node)| node.manifest_path.as_std_path() == abs)
        else {
            anyhow::bail!(
                "Could not find workspace member with manifest path {}",
                manifest_path.as_ref().display()
            )
        };

        let mut filtered_packages: HashSet<PackageId> = HashSet::default();
        let mut stack = vec![id];

        while let Some(id) = stack.pop() {
            let Some(pkg) = self.graph.get(id) else {
                continue;
            };

            for descendant in pkg.dependencies.iter() {
                if !filtered_packages.contains(&descendant.id) {
                    stack.push(&descendant.id);
                }
            }

            filtered_packages.insert(id.clone());
        }

        self.graph.retain(|id, _| filtered_packages.contains(id));

        Ok(())
    }

    /// Lowers the graph to a vector of crates
    fn lower_to_crates(
        self,
        proc_macro_dylibs: HashMap<PackageId, FilePathBuf>,
        build_scripts: HashMap<PackageId, BuildScript>,
    ) -> Result<Vec<Crate>> {
        let iter = self.graph.into_iter().flat_map(|(id, package)| {
            // TODO: clones
            package
                .clone()
                .targets
                .into_iter()
                .map(move |target| (id.clone(), package.clone(), target))
        });

        let mut crates = Vec::new();
        let mut deps = Vec::new();
        let mut indexes: HashMap<PackageId, usize> = HashMap::new();

        for (id, package, target) in iter {
            let mut env = HashMap::new();
            let mut include_dirs = vec![package.manifest_path.parent().unwrap().to_string()];
            if let Some(script) = build_scripts.get(&id) {
                env.insert("OUT_DIR".into(), script.out_dir.to_string());

                if let Some(parent) = script.out_dir.parent() {
                    include_dirs.push(parent.to_string());
                    env.extend(script.env.clone().into_iter());
                }
            }

            let target_kind = TargetKind::new(&target.kind);
            if matches!(target_kind, TargetKind::Lib) {
                indexes.insert(id.clone(), crates.len());
            }

            deps.push(package.dependencies);

            crates.push(Crate {
                display_name: Some(package.name.to_string().replace('-', "_")),
                root_module: target.root_module.clone(),
                edition: target.edition,
                version: Some(package.version.to_string()),
                deps: vec![],
                is_workspace_member: package.is_workspace_member,
                is_proc_macro: target.is_proc_macro(),
                repository: package.repository.clone(),
                build: Some(BuildInfo {
                    label: target.name.clone(),
                    build_file: package.manifest_path.to_string(),
                    target_kind,
                }),
                proc_macro_dylib_path: proc_macro_dylibs.get(&id).cloned(),
                source: Some(CrateSource {
                    include_dirs,
                    exclude_dirs: vec![".git".into(), "target".into()],
                }),
                // cfg_groups: None,
                cfg: package
                    .features
                    .clone()
                    .into_iter()
                    .map(|feature| format!("feature=\"{feature}\""))
                    .collect(),
                target: None,
                env,
                proc_macro_cwd: package
                    .manifest_path
                    .as_file_path()
                    .parent()
                    .map(|a| a.into()),
            });
        }

        for (c, deps) in crates.iter_mut().zip(deps.into_iter()) {
            c.deps = deps
                .into_iter()
                .map(|dep| Dep {
                    name: dep.name,
                    crate_index: indexes.get(&dep.id).copied().unwrap(),
                })
                .collect();

            // *shrug* buck does this, not sure if it's necessary
            c.deps.sort_by_key(|dep| dep.crate_index);
        }

        Ok(crates)
    }
}

fn crates_from_metadata(
    ctx: &Context,
    metadata: Metadata,
    manifest_path: FilePath<'_>,
) -> Result<Vec<Crate>> {
    #[cfg(not(target_os = "windows"))]
    let pprof_guard = {
        ctx.flamegraph
            .as_ref()
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

    let mut graph = PackageGraph::lower_from_metadata(metadata)?;
    let original_package_count = graph.graph.len();

    log_progress("Pruning metadata")?;
    graph.prune(manifest_path)?;

    debug!(
        original_package_count,
        new_package_count = graph.graph.len()
    );

    log_progress("Building proc macros")?;
    let (proc_macro_dylibs, build_scripts) =
        build_compile_time_dependencies(ctx, manifest_path, &graph.graph)?;

    log_progress("Constructing crate graph")?;
    let crates = graph.lower_to_crates(proc_macro_dylibs, build_scripts)?;

    #[cfg(not(target_os = "windows"))]
    if let Some((guard, path)) = pprof_guard {
        let report = guard.report().build()?;
        let file = File::create(path)?;

        report.flamegraph(file)?;
    }

    Ok(crates)
}
