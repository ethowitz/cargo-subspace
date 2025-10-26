mod utils;

use std::path::PathBuf;

use cargo_metadata::PackageId;
use cargo_subspace::{
    discover::DiscoverRunner,
    graph::{CrateGraph, Dependency, PackageNode},
};
use utils::prelude::*;

use crate::utils::{Crate, bin, lib, workspace};

// TEST STRATEGY
// - Snapshot testing (output doesn't change and is in expected format)
// - want to be able to inspect crate graph
//
// need to test
// - output format is expected (snapshot testing)
// - inspect crate graph for specific inputs
// - (stretch) RA returns correct diagnostics for specific inputs
//
// TODOS
// - make CARGO_HOME dynamic based on home (or something)
// - move move functionality into lib crate with public functions for discover_path,
// discover_buildfile that return pruned DepGraph

#[derive(Debug)]
struct GraphWrapper {
    graph: CrateGraph,
}

impl GraphWrapper {
    fn get(&self, c: &Crate) -> &PackageNode {
        let pkg = self.graph.inner.get(&c.package_id).unwrap();
        assert_eq!(c.name, pkg.name);
        assert_eq!(c.manifest_path(), pkg.manifest_path.as_std_path());
        assert!(pkg.is_workspace_member);

        pkg
    }

    fn get_pkg(&self, package_id: &PackageId) -> &PackageNode {
        self.graph.inner.get(package_id).unwrap()
    }

    fn len(&self) -> usize {
        self.graph.inner.len()
    }
}

trait DiscoverRunnerExt {
    fn build(self) -> GraphWrapper;
}

impl DiscoverRunnerExt for DiscoverRunner {
    fn build(self) -> GraphWrapper {
        let graph = self.get_crate_graph().unwrap();
        GraphWrapper { graph }
    }
}

fn discover_path<T>(path: T) -> GraphWrapper
where
    T: Into<PathBuf>,
{
    let p: PathBuf = path.into();
    DiscoverRunner::from_src_file(p.try_into().unwrap())
        .unwrap()
        .build()
}

fn discover_path_builder<T>(path: T) -> DiscoverRunner
where
    T: Into<PathBuf>,
{
    let p: PathBuf = path.into();
    DiscoverRunner::from_src_file(p.try_into().unwrap()).unwrap()
}

#[cargo_test]
fn standalone_crate() {
    let p = bin("foo")
        .file(
            "src/main.rs",
            stringify! {
                fn main() {
                    println!("hi!");
                }
            },
        )
        .build();

    let crate_graph = discover_path(p.file("src/main.rs"));

    assert_eq!(crate_graph.len(), 1);

    crate_graph.get(&p);
}

#[cargo_test]
fn test_workspace_with_independent_members() {
    let bar = lib("bar");
    let foo = lib("foo");
    let w = workspace().member(foo).member(bar).build();

    let crate_graph = discover_path(w.file("bar/src/lib.rs"));

    // We only ran discover on bar, so foo should not be in the graph
    assert_eq!(crate_graph.len(), 1);

    let bar = w.member("bar");
    let pkg = crate_graph.get(bar);
    assert!(pkg.dependencies.is_empty());

    let crate_graph = discover_path(w.file("foo/src/lib.rs"));

    // We only ran discover on foo, so bar should not be in the graph
    assert_eq!(crate_graph.graph.inner.len(), 1);

    let foo = w.member("foo");
    let pkg = crate_graph.get(foo);
    assert!(pkg.dependencies.is_empty());
}

#[cargo_test]
fn test_workspace_with_dependent_members() {
    // foo --> bar
    let bar = lib("bar");
    let foo = lib("foo").path_dependency(&bar);
    let w = workspace().member(foo).member(bar).build();

    let crate_graph = discover_path(w.file("bar/src/lib.rs"));

    // We only ran discover on bar, so foo should not be in the graph
    assert_eq!(crate_graph.len(), 1);

    let bar = w.member("bar");
    let pkg = crate_graph.get(bar);
    assert!(pkg.dependencies.is_empty());

    let crate_graph = discover_path(w.file("foo/src/lib.rs"));

    // We ran discover on foo, so both foo AND bar should be in the graph
    assert_eq!(crate_graph.graph.inner.len(), 2);

    let bar = w.member("bar");
    let pkg = crate_graph.get(bar);
    assert!(pkg.dependencies.is_empty());

    let foo = w.member("foo");
    let pkg = crate_graph.get(foo);
    assert_eq!(pkg.dependencies, vec![bar.as_dependency()]);
}

#[cargo_test]
fn test_workspace_with_shared_dependencies() {
    // c1 --> c3
    // c2 --> c3
    let c3 = lib("c3");
    let c2 = lib("c2").path_dependency(&c3);
    let c1 = lib("c1").path_dependency(&c3);
    let w = workspace().member(c1).member(c2).member(c3).build();

    let crate_graph = discover_path(w.file("c3/src/lib.rs"));

    // We only ran discover on c3, so c1 and c2 should not be in the graph
    assert_eq!(crate_graph.len(), 1);

    let c3 = w.member("c3");
    let pkg = crate_graph.get(c3);
    assert!(pkg.dependencies.is_empty());

    let crate_graph = discover_path(w.file("c1/src/lib.rs"));

    // We ran discover on c1, so both c1 AND c3 should be in the graph
    assert_eq!(crate_graph.graph.inner.len(), 2);

    let c3 = w.member("c3");
    let pkg = crate_graph.get(c3);
    assert!(pkg.dependencies.is_empty());

    let c1 = w.member("c1");
    let pkg = crate_graph.get(c1);
    assert_eq!(pkg.dependencies, vec![c3.as_dependency()]);

    let crate_graph = discover_path(w.file("c2/src/lib.rs"));

    // We ran discover on c2, so both c2 AND c3 should be in the graph
    assert_eq!(crate_graph.graph.inner.len(), 2);

    let c3 = w.member("c3");
    let pkg = crate_graph.get(c3);
    assert!(pkg.dependencies.is_empty());

    let c2 = w.member("c2");
    let pkg = crate_graph.get(c2);
    assert_eq!(pkg.dependencies, vec![c3.as_dependency()]);
}

#[cargo_test]
fn standalone_crate_with_external_dependency() {
    let mut builder = bin("foo");
    let hashbrown_id = builder.dependency("hashbrown", "0.16.0", true);
    let foo = builder.build();

    let crate_graph = discover_path_builder(foo.file("src/main.rs"))
        .with_no_default_features()
        .build();

    assert_eq!(crate_graph.len(), 2);

    let hashbrown = crate_graph.get_pkg(&hashbrown_id);
    assert_eq!(hashbrown.name, "hashbrown");
    assert_eq!(hashbrown.version.to_string(), "0.16.0");
    assert!(hashbrown.dependencies.is_empty());

    let foo = crate_graph.get(&foo);
    assert_eq!(
        foo.dependencies,
        vec![Dependency {
            id: hashbrown_id,
            name: "hashbrown".into(),
        }]
    )
}

#[cargo_test]
fn workspace_with_external_dependency() {
    // foo --> hashbrown
    // bar
    let mut foo = lib("foo");
    let hashbrown_id = foo.dependency("hashbrown", "0.16.0", true);
    let bar = lib("bar");
    let w = workspace().member(foo).member(bar).build();

    let crate_graph = discover_path_builder(w.file("foo/src/lib.rs"))
        .with_no_default_features()
        .build();

    assert_eq!(crate_graph.len(), 2);

    let hashbrown = crate_graph.get_pkg(&hashbrown_id);
    assert_eq!(hashbrown.name, "hashbrown");
    assert_eq!(hashbrown.version.to_string(), "0.16.0");
    assert!(hashbrown.dependencies.is_empty());

    let foo = crate_graph.get(w.member("foo"));
    assert_eq!(
        foo.dependencies,
        vec![Dependency {
            id: hashbrown_id,
            name: "hashbrown".into(),
        }]
    );

    let crate_graph = discover_path_builder(w.file("bar/src/lib.rs"))
        .with_no_default_features()
        .build();

    assert_eq!(crate_graph.len(), 1);

    let foo = crate_graph.get(w.member("bar"));
    assert!(foo.dependencies.is_empty());
}

#[cargo_test]
fn workspace_with_path_and_external_dependency() {
    // foo --> hashbrown, bar
    // bar
    let bar = lib("bar");
    let mut foo = lib("foo").path_dependency(&bar);
    let hashbrown_id = foo.dependency("hashbrown", "0.16.0", true);
    let w = workspace().member(foo).member(bar).build();

    let crate_graph = discover_path_builder(w.file("bar/src/lib.rs"))
        .with_no_default_features()
        .build();

    assert_eq!(crate_graph.len(), 1);

    let foo = crate_graph.get(w.member("bar"));
    assert!(foo.dependencies.is_empty());

    let crate_graph = discover_path_builder(w.file("foo/src/lib.rs"))
        .with_no_default_features()
        .build();

    assert_eq!(crate_graph.len(), 3);

    let hashbrown = crate_graph.get_pkg(&hashbrown_id);
    assert_eq!(hashbrown.name, "hashbrown");
    assert_eq!(hashbrown.version.to_string(), "0.16.0");
    assert!(hashbrown.dependencies.is_empty());

    let foo = crate_graph.get(w.member("foo"));

    assert_eq!(foo.dependencies.len(), 2);
    assert!(foo.dependencies.contains(&Dependency {
        id: hashbrown_id,
        name: "hashbrown".into(),
    }));
    assert!(foo.dependencies.contains(&w.member("bar").as_dependency()));
}

// All the existing tests but with external dependencies
// Build scripts/OUT_DIR
// Proc macros
// A standalone crate nested under the root of a workspace from which it's excluded
// Multiple build targets in a single crate
// Intra-crate target dependencies
// Feature flags (both ws + external deps)
