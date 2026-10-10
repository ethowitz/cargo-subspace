//! Tests for `cargo-subspace discover` on crates that depend on packages from a registry.
//!
//! These run the real binary and inspect the JSON that it reports to rust-analyzer. Registry
//! packages come from the fake local registry that `cargo-test-support` sets up in each test's
//! sandboxed `CARGO_HOME`, so the tests don't need network access. That sandbox is only applied to
//! processes spawned by the test, which is why these tests can't call `DiscoverRunner` in-process
//! the way the tests in `discover.rs` do.

mod utils;

use std::path::{Path, PathBuf};

use cargo_test_macro::cargo_test;
use cargo_test_support::registry::Package;
use serde_json::Value;

use crate::utils::{DiscoverOutput, bin, cfg, lib, workspace_of};

/// Publishes a package with the given name and version to the fake registry
fn publish(name: &str, version: &str) {
    Package::new(name, version).publish();
}

fn path(value: &Value) -> &Path {
    Path::new(value.as_str().unwrap())
}

#[cargo_test]
fn standalone_crate_with_external_dependency() {
    publish("hashbrown", "0.16.0");

    let mut foo = bin("foo");
    foo.dependency("hashbrown", "0.16.0", false);
    let foo = foo.build();

    let output = foo.discover();

    output.assert_packages(["foo", "hashbrown"]);
    output.assert_deps("foo", ["hashbrown"]);
    output.assert_deps("hashbrown", []);

    let (_, hashbrown) = output.find("hashbrown");
    assert_eq!(hashbrown["version"], "0.16.0");
    assert_eq!(hashbrown["is_workspace_member"], false);
    assert_eq!(output.find("foo").1["is_workspace_member"], true);
}

#[cargo_test]
fn workspace_with_independent_members_and_external_dependencies() {
    // foo --> hashbrown
    // bar --> anyhow
    publish("hashbrown", "0.16.0");
    publish("anyhow", "1.0.100");

    let mut foo = lib("foo");
    foo.dependency("hashbrown", "0.16.0", false);
    let mut bar = lib("bar");
    bar.dependency("anyhow", "1.0.100", false);
    let w = workspace_of([foo, bar]);

    // bar and its external dependency should have been pruned
    w.discover("foo").assert_packages(["foo", "hashbrown"]);

    // foo and its external dependency should have been pruned
    w.discover("bar").assert_packages(["bar", "anyhow"]);
}

#[cargo_test]
fn workspace_with_dependent_members_and_external_dependencies() {
    // foo --> bar --> hashbrown
    publish("hashbrown", "0.16.0");

    let mut bar = lib("bar");
    bar.dependency("hashbrown", "0.16.0", false);
    let foo = lib("foo").path_dependency(&bar);
    let w = workspace_of([foo, bar]);

    let output = w.discover("foo");

    // hashbrown is a dependency of bar only, so foo should not depend on it directly
    output.assert_packages(["foo", "bar", "hashbrown"]);
    output.assert_deps("foo", ["bar"]);
    output.assert_deps("bar", ["hashbrown"]);

    // foo is not a dependency of bar, so it should have been pruned
    w.discover("bar").assert_packages(["bar", "hashbrown"]);
}

#[cargo_test]
fn external_dependency_proc_macro_dylib() {
    Package::new("serde_derive", "1.0.228")
        .proc_macro(true)
        .publish();

    let mut foo = bin("foo");
    foo.dependency("serde_derive", "1.0.228", false);
    let foo = foo.build();

    let output = foo.discover();

    let (_, serde_derive) = output.find("serde_derive");
    assert_eq!(serde_derive["is_proc_macro"], true);

    utils::assert_proc_macro_dylib_path(
        path(&serde_derive["proc_macro_dylib_path"]),
        &foo.file("target"),
        "serde_derive",
    );

    // Only proc macros have a dylib
    assert!(output.find("foo").1.get("proc_macro_dylib_path").is_none());
}

#[cargo_test]
fn external_dependency_build_script() {
    Package::new("anyhow", "1.0.100")
        .file("src/lib.rs", "")
        .file(
            "build.rs",
            r#"
                fn main() {
                    println!("cargo:rustc-env=BUILD_SCRIPT_VAR=hello");
                }
            "#,
        )
        .publish();

    let mut foo = bin("foo");
    foo.dependency("anyhow", "1.0.100", false);
    let foo = foo.build();

    let output = foo.discover();

    // Each target of the package is reported as its own crate: the lib, and the build script
    assert_eq!(output.len(), 3);
    output.find_with_root_module("anyhow", "build.rs");
    let (_, anyhow) = output.find_with_root_module("anyhow", "src/lib.rs");
    assert_eq!(anyhow["env"]["BUILD_SCRIPT_VAR"], "hello");

    let out_dir = path(&anyhow["env"]["OUT_DIR"]);
    utils::assert_build_script_out_dir_path(out_dir, &foo.file("target"), "anyhow");
    let build_dir = out_dir.parent().unwrap();

    // The directory that contains OUT_DIR is added to the crate's source
    let include_dirs = anyhow["source"]["include_dirs"].as_array().unwrap();
    assert!(
        include_dirs.iter().any(|dir| path(dir) == build_dir),
        "{build_dir:?} not in {include_dirs:?}"
    );

    // foo has no build script of its own
    assert!(output.find("foo").1.get("env").is_none());
}

#[cargo_test]
#[ignore = "cfgs emitted by build scripts are not reported yet"]
fn build_script_cfgs_are_reported() {
    Package::new("anyhow", "1.0.100")
        .file("src/lib.rs", "")
        .file(
            "build.rs",
            r#"
                fn main() {
                    println!("cargo:rustc-check-cfg=cfg(from_build_script)");
                    println!("cargo:rustc-cfg=from_build_script");
                }
            "#,
        )
        .publish();

    let mut foo = bin("foo");
    foo.dependency("anyhow", "1.0.100", false);
    let foo = foo.build();

    let output = foo.discover();

    // rust-analyzer needs to know about the cfgs that the build script enables
    let (_, anyhow) = output.find_with_root_module("anyhow", "src/lib.rs");
    assert!(
        cfg(anyhow).contains(&"from_build_script".to_string()),
        "cfgs from the build script were not reported: {:?}",
        cfg(anyhow)
    );
}

#[cargo_test]
fn external_dependency_features() {
    // foo's default feature enables `fast`, which enables a feature of its external dependency
    Package::new("hashbrown", "0.16.0")
        .feature("inline-more", &[])
        .publish();

    let mut foo = lib("foo")
        .feature("default", ["fast"])
        .feature("fast", ["hashbrown/inline-more"]);
    foo.dependency("hashbrown", "0.16.0", false);
    let w = workspace_of([foo]);

    let default_features =
        |output: &DiscoverOutput| (cfg(output.find("foo").1), cfg(output.find("hashbrown").1));

    let (foo_cfg, hashbrown_cfg) = default_features(&w.discover("foo"));
    assert_eq!(foo_cfg, [r#"feature="default""#, r#"feature="fast""#]);
    assert_eq!(hashbrown_cfg, [r#"feature="inline-more""#]);

    let output = w.discover_with_flag("foo", "--no-default-features");
    let (foo_cfg, hashbrown_cfg) = default_features(&output);
    assert!(foo_cfg.is_empty());
    assert!(hashbrown_cfg.is_empty());

    let output = w.discover_with_flag("foo", "--all-features");
    let (foo_cfg, hashbrown_cfg) = default_features(&output);
    assert_eq!(foo_cfg, [r#"feature="default""#, r#"feature="fast""#]);
    assert_eq!(hashbrown_cfg, [r#"feature="inline-more""#]);
}

/// The value of an environment variable that was reported for a crate
fn env_var<'a>(krate: &'a Value, key: &str) -> Option<&'a str> {
    krate["env"].get(key).and_then(Value::as_str)
}

/// Asserts that the environment variables that cargo sets for the crates it compiles were reported
/// for the given crate.
///
/// rust-analyzer only sets these itself for the projects that it loads with cargo, not for
/// `rust-project.json` projects, so `env!("CARGO_PKG_VERSION")` and friends only resolve if they
/// are reported.
#[track_caller]
fn assert_cargo_package_env(
    krate: &Value,
    package_name: &str,
    (major, minor, patch): (u64, u64, u64),
) -> PathBuf {
    let expected = [
        ("CARGO_PKG_NAME", package_name.to_string()),
        ("CARGO_PKG_VERSION", format!("{major}.{minor}.{patch}")),
        ("CARGO_PKG_VERSION_MAJOR", major.to_string()),
        ("CARGO_PKG_VERSION_MINOR", minor.to_string()),
        ("CARGO_PKG_VERSION_PATCH", patch.to_string()),
        ("CARGO_PKG_VERSION_PRE", String::new()),
        // Unlike the package name, the crate name never has hyphens
        ("CARGO_CRATE_NAME", package_name.replace('-', "_")),
    ];

    for (key, value) in expected {
        assert_eq!(
            env_var(krate, key),
            Some(value.as_str()),
            "unexpected value for {key} in env: {}",
            krate["env"]
        );
    }

    let manifest_dir = PathBuf::from(
        env_var(krate, "CARGO_MANIFEST_DIR")
            .unwrap_or_else(|| panic!("CARGO_MANIFEST_DIR missing from env: {}", krate["env"])),
    );
    assert!(
        manifest_dir.join("Cargo.toml").is_file(),
        "CARGO_MANIFEST_DIR is not a package directory: {}",
        manifest_dir.display()
    );
    manifest_dir
}

#[cargo_test]
#[ignore = "CARGO_PKG_* environment variables are not reported yet"]
fn cargo_package_env_vars() {
    // foo-bar --> hashbrown
    publish("hashbrown", "0.16.0");

    // The hyphen distinguishes the package name from the crate name
    let mut foo_bar = bin("foo-bar");
    foo_bar.dependency("hashbrown", "0.16.0", false);
    let foo_bar = foo_bar.build();

    let output = foo_bar.discover();

    // A workspace member
    let (_, member) = output.find("foo_bar");
    let manifest_dir = assert_cargo_package_env(member, "foo-bar", (0, 1, 0));
    assert_eq!(manifest_dir, foo_bar.root);

    // A package from a registry
    let (_, hashbrown) = output.find("hashbrown");
    let manifest_dir = assert_cargo_package_env(hashbrown, "hashbrown", (0, 16, 0));
    assert!(
        manifest_dir.ends_with("hashbrown-0.16.0"),
        "unexpected manifest dir: {}",
        manifest_dir.display()
    );
}
