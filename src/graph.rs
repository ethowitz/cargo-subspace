use std::collections::{HashMap, HashSet};

use anyhow::Result;
use cargo_metadata::{BuildScript, Edition, Metadata, PackageId, semver::Version};

use crate::{
    rust_project::{BuildInfo, Crate, CrateSource, Dep, TargetKind},
    util::{FilePath, FilePathBuf},
};

#[derive(Debug)]
pub struct CrateGraph {
    pub inner: HashMap<PackageId, PackageNode>,
}

impl CrateGraph {
    pub fn from_metadata(metadata: Metadata) -> Result<Self> {
        let mut inner = HashMap::new();
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
                build_script: None,
                proc_macro_dylib: None,
            };

            inner.insert(package.id, node);
        }

        Ok(Self { inner })
    }

    pub fn get_mut(&mut self, id: &PackageId) -> Option<&mut PackageNode> {
        self.inner.get_mut(id)
    }

    /// Prunes the graph such that the remaining nodes consist only of:
    /// 1. The package with the given manifest path; and
    /// 2. The dependencies of that package
    pub fn prune(&mut self, manifest_path: FilePath<'_>) -> Result<()> {
        let abs = std::path::absolute(manifest_path.as_std_path())?;
        let Some((id, _)) = self
            .inner
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
            let Some(pkg) = self.inner.get(id) else {
                continue;
            };

            for descendant in pkg.dependencies.iter() {
                if !filtered_packages.contains(&descendant.id) {
                    stack.push(&descendant.id);
                }
            }

            filtered_packages.insert(id.clone());
        }

        self.inner.retain(|id, _| filtered_packages.contains(id));

        Ok(())
    }

    // TODO: get rid of PackageNode and just use Crate
    /// Converts the graph to a [`Vec<Crate>`]
    pub fn into_crates(self) -> Result<Vec<Crate>> {
        let mut crates = Vec::new();
        let mut deps = Vec::new();
        let mut indexes: HashMap<PackageId, usize> = HashMap::new();

        // test and bin targets under a package should depend on the lib targets

        for (id, package) in self.inner {
            let mut env = HashMap::new();
            let mut include_dirs = vec![package.manifest_path.parent().unwrap().to_string()];
            if let Some(script) = package.build_script {
                env.insert("OUT_DIR".into(), script.out_dir.to_string());

                if let Some(parent) = script.out_dir.parent() {
                    include_dirs.push(parent.to_string());
                    env.extend(script.env.clone().into_iter());
                }
            }

            let lib_deps = package
                .targets
                .iter()
                .enumerate()
                .filter_map(|(i, target)| {
                    let kind = TargetKind::new(&target.kind);

                    if matches!(kind, TargetKind::Lib) {
                        Some(Dep {
                            crate_index: crates.len() + i,
                            name: target.name.clone(),
                        })
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            for target in package.targets {
                deps.push(package.dependencies.clone());

                let target_kind = TargetKind::new(&target.kind);
                // TODO: this is not quite right
                if matches!(target_kind, TargetKind::Lib) {
                    indexes.insert(id.clone(), crates.len());
                }

                crates.push(Crate {
                    display_name: Some(package.name.to_string().replace('-', "_")),
                    root_module: target.root_module.clone(),
                    edition: target.edition,
                    version: Some(package.version.to_string()),
                    deps: if !matches!(target_kind, TargetKind::Lib) {
                        lib_deps.clone()
                    } else {
                        vec![]
                    },
                    is_workspace_member: package.is_workspace_member,
                    is_proc_macro: target.is_proc_macro(),
                    repository: package.repository.clone(),
                    build: Some(BuildInfo {
                        label: target.name.clone(),
                        build_file: package.manifest_path.to_string(),
                        target_kind,
                    }),
                    proc_macro_dylib_path: package.proc_macro_dylib.clone(),
                    source: Some(CrateSource {
                        include_dirs: include_dirs.clone(),
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
                    env: env.clone(),
                    proc_macro_cwd: package
                        .manifest_path
                        .as_file_path()
                        .parent()
                        .map(|a| a.into()),
                });
            }
        }

        for (c, deps) in crates.iter_mut().zip(deps.into_iter()) {
            c.deps.extend(deps.into_iter().map(|dep| Dep {
                name: dep.name,
                crate_index: indexes.get(&dep.id).copied().unwrap(),
            }));

            // *shrug* buck does this, not sure if it's necessary
            c.deps.sort_by_key(|dep| dep.crate_index);
        }

        Ok(crates)
    }
}

/// Represents one target of a single package
#[derive(Clone, Debug)]
pub struct PackageNode {
    pub name: String,
    pub targets: Vec<Target>,
    pub manifest_path: FilePathBuf,
    pub version: Version,
    pub is_workspace_member: bool,
    pub repository: Option<String>,
    pub features: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub build_script: Option<BuildScript>,
    pub proc_macro_dylib: Option<FilePathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub id: PackageId,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Target {
    pub name: String,
    pub edition: Edition,
    pub kind: Vec<cargo_metadata::TargetKind>,
    pub root_module: FilePathBuf,
}

impl Target {
    fn is_proc_macro(&self) -> bool {
        self.kind
            .iter()
            .any(|k| matches!(k, cargo_metadata::TargetKind::ProcMacro))
    }
}
