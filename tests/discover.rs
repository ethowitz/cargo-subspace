//! Tests for `cargo-subspace discover`. Most of these run the discover process in-process and
//! inspect the resulting crate graph.

mod utils;

use cargo_metadata::TargetKind as CargoTargetKind;
use cargo_subspace::{graph::PackageNode, rust_project::Crate as ProjectCrate};
use utils::prelude::*;

use crate::utils::{DiscoverRunnerExt, lib, workspace, workspace_of};

/// The sorted (name, kind) pairs of the targets of the given package
fn target_names_and_kinds(pkg: &PackageNode) -> Vec<(String, CargoTargetKind)> {
    sorted(
        pkg.targets
            .iter()
            .map(|target| (target.name.clone(), target.kind[0].clone()))
            .collect(),
    )
}

fn sorted<T: Ord>(mut items: Vec<T>) -> Vec<T> {
    items.sort();
    items
}

/// The sorted features that were activated for the given package
fn features(pkg: &PackageNode) -> Vec<String> {
    let mut features = pkg.features.clone();
    features.sort();
    features
}

/// The index of the crate with the given display name and a root module ending with the given path
fn crate_index(crates: &[ProjectCrate], display_name: &str, root_module: &str) -> usize {
    let matches: Vec<_> = crates
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.display_name.as_deref() == Some(display_name)
                && c.root_module.as_std_path().ends_with(root_module)
        })
        .map(|(i, _)| i)
        .collect();

    assert_eq!(
        matches.len(),
        1,
        "expected exactly one crate `{display_name}` with root module {root_module}"
    );
    matches[0]
}

/// The (crate index, name) pairs of a crate's dependencies
fn deps(c: &ProjectCrate) -> Vec<(usize, String)> {
    c.deps
        .iter()
        .map(|dep| (dep.crate_index, dep.name.clone()))
        .collect()
}

#[cargo_test]
fn test_workspace_with_independent_members() {
    let w = workspace_of([lib("foo"), lib("bar")]);

    // Only the package that discover ran on is in the graph
    let graph = w.graph("bar");
    graph.assert_packages(["bar"]);
    graph.assert_deps("bar", []);

    let graph = w.graph("foo");
    graph.assert_packages(["foo"]);
    graph.assert_deps("foo", []);
}

#[cargo_test]
fn test_workspace_with_dependent_members() {
    // foo --> bar
    let bar = lib("bar");
    let foo = lib("foo").path_dependency(&bar);
    let w = workspace_of([foo, bar]);

    // foo is not a dependency of bar, so it is not in the graph
    let graph = w.graph("bar");
    graph.assert_packages(["bar"]);
    graph.assert_deps("bar", []);

    let graph = w.graph("foo");
    graph.assert_packages(["foo", "bar"]);
    graph.assert_deps("foo", ["bar"]);
    graph.assert_deps("bar", []);
}

#[cargo_test]
fn test_workspace_with_shared_dependencies() {
    // c1 --> c3
    // c2 --> c3
    let c3 = lib("c3");
    let c2 = lib("c2").path_dependency(&c3);
    let c1 = lib("c1").path_dependency(&c3);
    let w = workspace_of([c1, c2, c3]);

    // Neither c1 nor c2 is a dependency of c3
    let graph = w.graph("c3");
    graph.assert_packages(["c3"]);
    graph.assert_deps("c3", []);

    // Each of them has the shared dependency, but not the other one
    for member in ["c1", "c2"] {
        let graph = w.graph(member);
        graph.assert_packages([member, "c3"]);
        graph.assert_deps(member, ["c3"]);
        graph.assert_deps("c3", []);
    }
}

#[cargo_test]
fn standalone_crate_excluded_from_workspace() {
    // The standalone crate is nested under the root of the workspace, but is not a member of it
    let w = workspace()
        .member(lib("member"))
        .excluded(lib("standalone"))
        .build();

    // The member is not a dependency of the standalone crate, and the other way around
    let graph = w.graph("standalone");
    graph.assert_packages(["standalone"]);
    graph.get(w.excluded("standalone"));

    let graph = w.graph("member");
    graph.assert_packages(["member"]);
    graph.get(w.member("member"));
}

#[cargo_test]
fn multiple_targets_in_a_single_crate() {
    let foo = lib("foo")
        .file("src/main.rs", "fn main() {}")
        .file("src/bin/tool.rs", "fn main() {}")
        .file("tests/integration.rs", "")
        .file("examples/demo.rs", "fn main() {}")
        .file("benches/speed.rs", "");
    let w = workspace_of([foo]);

    let graph = w.graph("foo");

    graph.assert_packages(["foo"]);
    assert_eq!(
        target_names_and_kinds(graph.package("foo")),
        sorted(vec![
            ("foo".to_string(), CargoTargetKind::Lib),
            ("foo".to_string(), CargoTargetKind::Bin),
            ("tool".to_string(), CargoTargetKind::Bin),
            ("integration".to_string(), CargoTargetKind::Test),
            ("demo".to_string(), CargoTargetKind::Example),
            ("speed".to_string(), CargoTargetKind::Bench),
        ])
    );

    // Every target is reported as its own crate
    assert_eq!(graph.into_crates().len(), 6);
}

#[cargo_test]
fn workspace_member_features() {
    // foo has default features, bar has an independent feature
    let foo = lib("foo")
        .feature("default", ["a"])
        .feature("a", [] as [&str; 0])
        .feature("b", [] as [&str; 0]);
    let bar = lib("bar").feature("x", [] as [&str; 0]);
    let w = workspace_of([foo, bar]);

    let graph = w.runner("foo").with_default_features().build();
    assert_eq!(features(graph.package("foo")), ["a", "default"]);

    let graph = w.runner("foo").with_no_default_features().build();
    assert!(features(graph.package("foo")).is_empty());

    let graph = w.runner("foo").with_all_features().build();
    assert_eq!(features(graph.package("foo")), ["a", "b", "default"]);

    // Feature flags apply to the whole workspace, not just the crate discover was run on
    let graph = w.runner("bar").with_all_features().build();
    assert_eq!(features(graph.package("bar")), ["x"]);
}

#[cargo_test]
fn other_targets_depend_on_the_lib_target_of_their_package() {
    // foo-bar has a lib, two bins, and a test, and depends on baz
    let baz = lib("baz");
    let foo_bar = lib("foo-bar")
        .file("src/main.rs", "fn main() {}")
        .file("src/bin/tool.rs", "fn main() {}")
        .file("tests/integration.rs", "")
        .path_dependency(&baz);
    let w = workspace_of([foo_bar, baz]);

    let crates = w.graph("foo-bar").into_crates();
    assert_eq!(crates.len(), 5);

    let baz_index = crate_index(&crates, "baz", "baz/src/lib.rs");
    let lib_index = crate_index(&crates, "foo_bar", "foo-bar/src/lib.rs");

    // The lib target doesn't depend on itself
    assert_eq!(
        deps(&crates[lib_index]),
        vec![(baz_index, "baz".to_string())]
    );

    // Every other target in the package can use the items defined in the lib target, which is
    // imported by its name (where hyphens in the name of the package become underscores), and keeps
    // the dependencies of the package
    for root_module in [
        "foo-bar/src/main.rs",
        "foo-bar/src/bin/tool.rs",
        "foo-bar/tests/integration.rs",
    ] {
        let index = crate_index(&crates, "foo_bar", root_module);
        assert_eq!(
            sorted(deps(&crates[index])),
            sorted(vec![
                (baz_index, "baz".to_string()),
                (lib_index, "foo_bar".to_string()),
            ]),
            "unexpected dependencies for {root_module}"
        );
    }
}
