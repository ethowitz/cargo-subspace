use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use cargo_metadata::PackageId;
use cargo_test_support::{
    ArgLineCommandExt, Execs, execs,
    paths::{self, CargoPathExt},
};

pub mod prelude {
    pub use super::CargoSubspaceExt;
    pub use cargo_test_support::prelude::*;
}

pub trait CargoSubspaceExt {
    // TODO: split this into discover_path, discover_buildfile, and check
    fn cargo_subspace(&self, cmd: &str) -> Execs;
}

fn process(root: &Path) -> Execs {
    let program = snapbox::cmd::cargo_bin!("cargo-subspace").to_path_buf();
    let mut p = cargo_test_support::process(program);
    p.cwd(root);
    execs().with_process_builder(p)
}

impl CargoSubspaceExt for Crate {
    fn cargo_subspace(&self, cmd: &str) -> Execs {
        let mut execs = process(&self.root);
        execs.arg_line(cmd);
        // TODO
        execs.env("CARGO_HOME", "/Users/ethan/.cargo/bin");
        execs
    }
}

impl CargoSubspaceExt for Workspace {
    fn cargo_subspace(&self, cmd: &str) -> Execs {
        let mut execs = process(&self.root);
        execs.arg_line(cmd);
        // TODO
        execs.env("CARGO_HOME", "/Users/ethan/.cargo/bin");
        execs
    }
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

// trait FixIt {
//     fn mheyyy(&self, cmd: &str);
// }
//
// impl FixIt for Project {
//     fn mheyyy(&self, cmd: &str) {
//         let fixed = true;
//         let program = fixed;
//     }
// }

pub enum Dependency {
    Path(PathBuf),
    External {
        version: String,
        no_default_features: bool,
    },
}

pub struct ExternalDep {
    package_id: String,
    version: String,
}

pub struct Crate {
    pub name: String,
    root: PathBuf,
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
        }
    }

    pub fn dependency<T, S>(&mut self, name: T, version: S, default_features: bool) -> PackageId
    where
        T: Into<String>,
        S: Into<String>,
    {
        let name = name.into();
        let version = version.into();

        let package_id = PackageId {
            repr: format!("registry+https://github.com/rust-lang/crates.io-index#{name}@{version}"),
        };

        self.dependencies.insert(
            name,
            Dependency::External {
                version,
                no_default_features: default_features,
            },
        );

        package_id
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
        root.rm_rf();
        root.mkdir_p();

        let name = self.name.clone();
        self.build_with_root(&root);

        let package_id = PackageId {
            repr: format!("path+file://{}#{name}@0.1.0", root.display()),
        };

        Crate {
            root,
            name,
            package_id,
        }
    }

    fn build_with_root(self, root: &Path) {
        Self::create_cargo_toml(root, &self.name, self.dependencies, self.features);

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
    ) {
        let mut cargo_toml = format!(
            r#"
            [package]
            name = "{name}"
            version = "0.1.0"
            edition = "2024"

            [dependencies]
        "#,
        );

        for (name, dependency) in dependencies {
            let s = match dependency {
                Dependency::Path(path) => format!(r#"{name} = {{ path = "{}"}}"#, path.display()),
                Dependency::External {
                    version,
                    no_default_features: false,
                } => format!(r#"{name} = "{version}""#),
                Dependency::External {
                    version,
                    no_default_features: true,
                } => format!(r#"{name} = {{ version = "{version}", default-features = false }}"#),
            };

            cargo_toml.push('\n');
            cargo_toml.push_str(&s);
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

    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("Cargo.toml")
    }
}

pub fn workspace() -> WorkspaceBuilder {
    WorkspaceBuilder::new()
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
        root.rm_rf();
        root.mkdir_p();

        self.create_cargo_toml();

        let mut members = HashMap::new();
        let mut excludes = HashMap::new();

        for (crate_root, member) in self.members {
            let abs = root.join(crate_root);
            abs.mkdir_p();

            let name = member.name.clone();
            member.build_with_root(&abs);

            let package_id = PackageId {
                repr: format!("path+file://{}#0.1.0", abs.display()),
            };

            members.insert(
                name.clone(),
                Crate {
                    name,
                    root: abs,
                    package_id,
                },
            );
        }

        for (crate_root, exclude) in self.exclude {
            let abs = root.join(crate_root);
            abs.mkdir_p();

            let name = exclude.name.clone();
            exclude.build_with_root(&abs);

            let package_id = PackageId {
                repr: format!("path+file://{}#{}@0.1.0", root.display(), name),
            };

            excludes.insert(
                name.clone(),
                Crate {
                    name,
                    root: abs,
                    package_id,
                },
            );
        }

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

        let contents = format!(
            r#"
            [workspace]
            resolver = "3"
            members = [{members}]
            exclude = [{exclude}]
        "#
        );
        let path = paths::root().join("Cargo.toml");
        fs::write(&path, &contents)
            .unwrap_or_else(|e| panic!("could not create file {}: {}", path.display(), e));
    }
}
