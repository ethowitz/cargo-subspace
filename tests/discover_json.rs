//! Tests for the JSON that `cargo-subspace discover` reports to rust-analyzer, on workspaces of
//! local crates. These run the real binary.
//!
//! Every test that goes through [`DiscoverOutput::run`] (including the ones in
//! `discover_registry.rs`) also validates the output against the invariants that rust-analyzer
//! relies on when it loads a `rust-project.json`. The first tests in this file check that
//! validation itself, so that it can't silently stop catching things.
//!
//! The tests that are ignored describe behavior that rust-analyzer needs but that `discover`
//! doesn't have yet. They fail when run with `--ignored`, and each one should be un-ignored with
//! the change that fixes it.

mod utils;

use std::{fs, path::Path, process::Command};

use cargo_test_macro::cargo_test;
use cargo_test_support::paths;
use serde_json::{Value, json};
use snapbox::str;

use crate::utils::{CargoSubspaceExt, DiscoverOutput, discover_json, lib, workspace, workspace_of};

// ----------------------------------------------------------------------------------------------
// The validation of the output
// ----------------------------------------------------------------------------------------------

/// A crate whose files exist under `root`, and that passes validation
fn valid_crate(root: &Path, name: &str, deps: Value) -> Value {
    let root_module = root.join(format!("{name}.rs"));
    fs::write(&root_module, "").unwrap();
    let build_file = root.join("Cargo.toml");
    fs::write(&build_file, "").unwrap();

    json!({
        "display_name": name,
        "root_module": root_module,
        "edition": "2021",
        "deps": deps,
        "is_workspace_member": true,
        "is_proc_macro": false,
        "cfg": ["feature=\"default\"", "unix"],
        "env": { "KEY": "value" },
        "source": { "include_dirs": [root], "exclude_dirs": [] },
        "build": { "label": name, "build_file": build_file, "target_kind": "lib" },
    })
}

/// The crate at the given index of a `finished` message
fn krate(message: &mut Value, index: usize) -> &mut Value {
    &mut message["project"]["crates"][index]
}

/// Validates a `finished` message with two valid crates, where the first one depends on the second,
/// after the given change to it
fn validate_after(change: impl FnOnce(&mut Value)) {
    let root = paths::root();
    let mut message = json!({
        "kind": "finished",
        "buildfile": root.join("Cargo.toml"),
        "project": {
            "sysroot": root,
            "sysroot_src": root.join("lib/rustlib/src/rust/library"),
            "crates": [
                valid_crate(&root, "a", json!([{ "crate": 1, "name": "b" }])),
                valid_crate(&root, "b", json!([])),
            ],
            "runnables": [],
        },
    });

    change(&mut message);
    DiscoverOutput::from_message(message.to_string(), message).validate();
}

/// Declares a test for each way that a project can break a rule of the validation. The project is
/// valid except for the change that is made to it, which has to be rejected with a message that
/// contains the given text, so that each test is about the rule that it says it is about.
macro_rules! rejected {
    ($($name:ident: $message:literal, |$m:ident| $change:expr;)*) => {$(
        #[cargo_test]
        #[should_panic(expected = $message)]
        fn $name() {
            validate_after(|$m| {
                let () = $change;
            });
        }
    )*};
}

/// Declares a test for each way that a project can differ from the valid one and still pass the
/// validation, so that the rules don't reject more than they should.
macro_rules! accepted {
    ($($name:ident: |$m:ident| $change:expr;)*) => {$(
        #[cargo_test]
        fn $name() {
            validate_after(|$m| {
                let () = $change;
            });
        }
    )*};
}

mod validation_rejects {
    use super::*;

    rejected! {
        // The message
        a_message_that_is_not_finished: "not a `finished` message", |m| m["kind"] = json!("error");
        a_relative_buildfile: "buildfile is not an absolute path", |m| m["buildfile"] = json!("Cargo.toml");
        a_relative_sysroot: "sysroot is not an absolute path", |m| m["project"]["sysroot"] = json!("sysroot");
        a_sysroot_that_does_not_exist: "sysroot does not exist", |m| m["project"]["sysroot"] = json!(paths::root().join("missing"));
        a_relative_sysroot_src: "sysroot_src is not an absolute path", |m| m["project"]["sysroot_src"] = json!("library");
        no_crates: "the project has no crates", |m| m["project"]["crates"] = json!([]);

        // The files of a crate
        a_relative_root_module: "root_module is not an absolute path", |m| krate(m, 0)["root_module"] = json!("src/lib.rs");
        a_root_module_that_does_not_exist: "root module does not exist", |m| krate(m, 0)["root_module"] = json!(paths::root().join("missing.rs"));
        a_relative_include_dir: "include_dirs is not an absolute path", |m| krate(m, 0)["source"]["include_dirs"] = json!(["src"]);
        an_include_dir_that_does_not_exist: "include dir does not exist", |m| krate(m, 0)["source"]["include_dirs"] = json!([paths::root().join("missing")]);
        a_relative_proc_macro_dylib: "proc_macro_dylib_path is not an absolute path", |m| krate(m, 0)["proc_macro_dylib_path"] = json!("libmacro.so");
        a_proc_macro_dylib_that_does_not_exist: "dylib does not exist", |m| krate(m, 0)["proc_macro_dylib_path"] = json!(paths::root().join("missing.so"));

        // The other fields of a crate
        an_unknown_edition: "invalid edition", |m| krate(m, 0)["edition"] = json!("2027");
        an_is_proc_macro_that_is_not_a_bool: "is_proc_macro is not a bool", |m| krate(m, 0)["is_proc_macro"] = json!("yes");
        an_unknown_target_kind: "invalid target_kind", |m| krate(m, 0)["build"]["target_kind"] = json!("example");
        an_empty_build_label: "empty build label", |m| krate(m, 0)["build"]["label"] = json!("");
        a_relative_build_file: "build_file is not an absolute path", |m| krate(m, 0)["build"]["build_file"] = json!("Cargo.toml");
        a_build_file_that_does_not_exist: "build file does not exist", |m| krate(m, 0)["build"]["build_file"] = json!(paths::root().join("missing/Cargo.toml"));
        an_env_value_that_is_not_a_string: "KEY is not a string", |m| krate(m, 0)["env"] = json!({ "KEY": 1 });
        an_env_variable_without_a_name: "empty environment variable name", |m| krate(m, 0)["env"] = json!({ "": "value" });

        // Cfgs
        a_cfg_with_an_unquoted_value: "is not a valid cfg", |m| krate(m, 0)["cfg"] = json!(["feature=default"]);
        a_cfg_with_an_unterminated_value: "is not a valid cfg", |m| krate(m, 0)["cfg"] = json!(["feature=\"default"]);
        a_cfg_with_a_hyphen: "`my-cfg` is not a valid cfg", |m| krate(m, 0)["cfg"] = json!(["my-cfg"]);
        an_empty_cfg: "`` is not a valid cfg", |m| krate(m, 0)["cfg"] = json!([""]);

        // Dependencies
        deps_that_are_not_an_array: "deps is not an array", |m| krate(m, 0)["deps"] = json!({});
        a_dependency_without_a_crate: "dependency without a crate index", |m| krate(m, 0)["deps"] = json!([{ "name": "b" }]);
        a_dependency_outside_of_the_crates: "dependency on crate #5, but there are only 2 crates", |m| krate(m, 1)["deps"] = json!([{ "crate": 5, "name": "c" }]);
        a_dependency_on_itself: "depends on itself", |m| krate(m, 1)["deps"] = json!([{ "crate": 1, "name": "b" }]);
        two_dependencies_with_the_same_name: "more than one dependency named `dep`", |m| krate(m, 0)["deps"] = json!([{ "crate": 1, "name": "dep" }, { "crate": 1, "name": "dep" }]);
        a_dependency_name_with_a_hyphen: "`my-dep` is not a valid crate name", |m| krate(m, 0)["deps"] = json!([{ "crate": 1, "name": "my-dep" }]);
        an_empty_dependency_name: "`` is not a valid crate name", |m| krate(m, 0)["deps"] = json!([{ "crate": 1, "name": "" }]);
        a_dependency_cycle: "the crate graph has a dependency cycle: a", |m| krate(m, 1)["deps"] = json!([{ "crate": 0, "name": "a" }]);
        a_longer_dependency_cycle: "the crate graph has a dependency cycle", |m| {
            krate(m, 1)["deps"] = json!([{ "crate": 2, "name": "c" }]);
            let c = valid_crate(&paths::root(), "c", json!([{ "crate": 0, "name": "a" }]));
            m["project"]["crates"].as_array_mut().unwrap().push(c);
        };
    }
}

mod validation_accepts {
    use super::*;

    accepted! {
        the_valid_project: |_m| ();
        a_project_without_the_optional_fields: |m| {
            m["project"].as_object_mut().unwrap().remove("sysroot_src");
            for c in m["project"]["crates"].as_array_mut().unwrap() {
                for key in ["cfg", "env", "source", "build", "proc_macro_dylib_path"] {
                    c.as_object_mut().unwrap().remove(key);
                }
            }
        };
        cfgs_that_are_atoms_or_have_values: |m| krate(m, 0)["cfg"] = json!(["unix", "debug_assertions", "feature=\"a-b\"", "target_os=\"linux\""]);
        a_proc_macro_with_its_dylib: |m| {
            krate(m, 1)["is_proc_macro"] = json!(true);
            // Any file will do
            krate(m, 1)["proc_macro_dylib_path"] = json!(paths::root().join("b.rs"));
        };
        a_crate_that_depends_on_another_one_under_two_names: |m| krate(m, 0)["deps"] = json!([{ "crate": 1, "name": "b" }, { "crate": 1, "name": "b2" }]);
        a_dependency_that_is_shared: |m| {
            // a --> b --> d
            // a --> c --> d
            let root = paths::root();
            krate(m, 0)["deps"] = json!([{ "crate": 1, "name": "b" }, { "crate": 2, "name": "c" }]);
            krate(m, 1)["deps"] = json!([{ "crate": 3, "name": "d" }]);
            let crates = m["project"]["crates"].as_array_mut().unwrap();
            crates.push(valid_crate(&root, "c", json!([{ "crate": 3, "name": "d" }])));
            crates.push(valid_crate(&root, "d", json!([])));
        };
    }
}

// ----------------------------------------------------------------------------------------------
// Behavior that works today
// ----------------------------------------------------------------------------------------------

#[cargo_test]
fn project_has_the_fields_that_rust_analyzer_needs() {
    let foo = lib("foo").build();

    let output = foo.discover();

    // The build file identifies the project to rust-analyzer, which rediscovers it with this
    // path when it changes
    assert_eq!(output.buildfile(), foo.file("Cargo.toml").to_str().unwrap());

    // The sysroot is the one of the toolchain that is being used
    let sysroot = Command::new("rustc")
        .args(["--print", "sysroot"])
        .output()
        .unwrap()
        .stdout;
    let sysroot = String::from_utf8(sysroot).unwrap();
    let sysroot = sysroot.trim();

    let project = output.project();
    assert_eq!(project["sysroot"], sysroot);
    assert_eq!(
        project["sysroot_src"],
        Path::new(sysroot)
            .join("lib/rustlib/src/rust/library")
            .to_str()
            .unwrap()
    );

    // Each crate refers back to the build file, so that rust-analyzer can find the crate that a
    // saved build file belongs to
    let (_, krate) = output.find("foo");
    assert_eq!(
        krate["build"]["build_file"],
        foo.file("Cargo.toml").to_str().unwrap()
    );
}

#[cargo_test]
fn edition_is_reported_per_package() {
    // foo --> bar --> baz --> qux, each of a different edition
    let qux = lib("qux").edition("2015");
    let baz = lib("baz").edition("2018").path_dependency(&qux);
    let bar = lib("bar").edition("2021").path_dependency(&baz);

    // The 2024 edition is the one that new projects use, but cargo can only build it since 1.85
    let newest = if utils::supports_edition_2024() {
        "2024"
    } else {
        "2021"
    };
    let foo = lib("foo").edition(newest).path_dependency(&bar);
    let w = workspace_of([foo, bar, baz, qux]);

    let output = w.discover("foo");

    output.assert_packages(["foo", "bar", "baz", "qux"]);
    assert_eq!(output.find("foo").1["edition"], newest);
    assert_eq!(output.find("bar").1["edition"], "2021");
    assert_eq!(output.find("baz").1["edition"], "2018");
    assert_eq!(output.find("qux").1["edition"], "2015");
}

#[cargo_test]
fn renamed_dependencies_are_imported_by_their_alias() {
    // app --> my-dep (as `renamed`), third (as `other-alias`)
    let my_dep = lib("my-dep");
    let third = lib("third");
    let app = lib("app")
        .raw_dependency("renamed", r#"{ path = "../my-dep", package = "my-dep" }"#)
        .raw_dependency("other-alias", r#"{ path = "../third", package = "third" }"#);
    let w = workspace_of([app, my_dep, third]);

    let output = w.discover("app");

    let (my_dep_index, _) = output.find("my_dep");
    let (third_index, _) = output.find("third");
    let (_, app) = output.find("app");

    // Hyphens in an alias become underscores, like they do in the name of the crate that is
    // imported
    let mut expected = vec![
        (my_dep_index, "renamed".to_string()),
        (third_index, "other_alias".to_string()),
    ];
    expected.sort();
    assert_eq!(output.deps(app), expected);
}

#[cargo_test]
fn lib_target_name_is_used_to_import_the_lib() {
    // app --> my-pkg, whose lib target is called `customlib`, and which also has a bin
    let my_pkg = lib("my-pkg")
        .manifest("[lib]\nname = \"customlib\"")
        .file("src/main.rs", "fn main() {}");
    let app = lib("app").path_dependency(&my_pkg);
    let w = workspace_of([app, my_pkg]);

    let output = w.discover("app");

    let (lib_index, _) = output.find_with_root_module("my_pkg", "src/lib.rs");
    let (_, bin) = output.find_with_root_module("my_pkg", "src/main.rs");
    let (_, app) = output.find("app");

    // The lib is imported by the name of its target, not the name of its package, both by the
    // bin in the same package and by other packages
    assert_eq!(output.deps(bin), vec![(lib_index, "customlib".to_string())]);
    assert_eq!(output.deps(app), vec![(lib_index, "customlib".to_string())]);
}

#[cargo_test]
fn platform_specific_dependencies_for_other_platforms_are_excluded() {
    // app --> hostdep (on this platform only), otherdep (on every other platform)
    let os = std::env::consts::OS;
    let hostdep = lib("hostdep");
    let otherdep = lib("otherdep");
    let app = lib("app").manifest(format!(
        r#"
            [target.'cfg(target_os = "{os}")'.dependencies]
            hostdep = {{ path = "../hostdep" }}

            [target.'cfg(not(target_os = "{os}"))'.dependencies]
            otherdep = {{ path = "../otherdep" }}
        "#
    ));
    let w = workspace_of([app, hostdep, otherdep]);

    let output = w.discover("app");

    output.assert_packages(["app", "hostdep"]);
    output.assert_deps("app", ["hostdep"]);
}

/// Creates a symlink to a directory
#[cfg(unix)]
fn symlink_dir(original: &Path, link: &Path) {
    std::os::unix::fs::symlink(original, link).unwrap();
}

#[cfg(windows)]
fn symlink_dir(original: &Path, link: &Path) {
    std::os::windows::fs::symlink_dir(original, link).unwrap();
}

#[cargo_test(ignore_windows = "creating a symlink needs a privilege that isn't granted by default")]
fn path_through_a_symlink_is_reported_as_given() {
    let foo = lib("foo").build();

    // A symlink to the directory of the crate, next to it
    let link = foo.root.with_file_name(format!(
        "{}-link",
        foo.root.file_name().unwrap().to_str().unwrap()
    ));
    let _ = fs::remove_file(&link);
    symlink_dir(&foo.root, &link);

    // rust-analyzer compares the build file in the output with the path of the file that was
    // saved, so the path can't be resolved to the one that the symlink points to
    let by_buildfile = discover_json(&foo, link.join("Cargo.toml"));
    let by_source = DiscoverOutput::run(&mut foo.discover_src(link.join("src/lib.rs")));
    fs::remove_file(&link).unwrap();

    for output in [by_buildfile, by_source] {
        assert_eq!(
            output.buildfile(),
            link.join("Cargo.toml").to_str().unwrap()
        );

        let (_, krate) = output.find("foo");
        assert_eq!(
            krate["root_module"],
            link.join("src/lib.rs").to_str().unwrap()
        );
        assert_eq!(
            krate["build"]["build_file"],
            link.join("Cargo.toml").to_str().unwrap()
        );
    }
}

#[cargo_test]
fn build_file_that_is_not_a_package_is_reported_as_an_error() {
    let w = workspace_of([lib("foo")]);

    // The manifest of a virtual workspace doesn't belong to any package
    w.discover_manifest(&w.manifest_path())
        .with_status(1)
        .with_stdout_data(str![[r#"
...
{"kind":"error","error":"Could not find workspace member with manifest path [..]","source":null}

"#]])
        .run();
}

/// A workspace in which the build of some compile-time dependencies fails:
///
/// app (whose build script panics) --> pm (a proc macro that doesn't compile), healthy (whose
/// build script works)
fn workspace_with_failing_builds() -> utils::Workspace {
    let pm = lib("pm")
        .proc_macro()
        .file("src/lib.rs", r#"compile_error!("pm is broken");"#);
    let healthy = lib("healthy").file(
        "build.rs",
        r#"
            fn main() {
                println!("cargo:rustc-env=HEALTHY=yes");
            }
        "#,
    );
    let app = lib("app")
        .path_dependency(&pm)
        .path_dependency(&healthy)
        .file(
            "build.rs",
            r#"fn main() { panic!("the build script is broken"); }"#,
        );

    workspace_of([app, pm, healthy])
}

#[cargo_test]
fn failing_builds_of_compile_time_dependencies_do_not_fail_discover() {
    let w = workspace_with_failing_builds();

    // rust-analyzer shows no project at all when discovery fails, which is the worst time for
    // that to happen while editing a build script or a proc macro
    let output = w.discover("app");

    // The crates are still there, along with their dependencies
    let (pm_index, pm) = output.find("pm");
    let (healthy_index, healthy) = output.find_with_root_module("healthy", "src/lib.rs");
    let (_, app_lib) = output.find_with_root_module("app", "src/lib.rs");
    output.find_with_root_module("app", "build.rs");
    let mut expected = vec![
        (pm_index, "pm".to_string()),
        (healthy_index, "healthy".to_string()),
    ];
    expected.sort();
    assert_eq!(output.deps(app_lib), expected);

    // What failed to build is missing from them: no output of the build script that panicked, and
    // no dylib for the proc macro that doesn't compile
    assert_eq!(pm["is_proc_macro"], true);
    assert!(pm.get("proc_macro_dylib_path").is_none_or(Value::is_null));
    assert!(app_lib.get("env").is_none_or(Value::is_null));

    // What didn't fail to build is not affected by the failures
    assert_eq!(healthy["env"]["HEALTHY"], "yes");
    assert!(healthy["env"]["OUT_DIR"].is_string());
}

// ----------------------------------------------------------------------------------------------
// Behavior that rust-analyzer needs but that doesn't exist yet
// ----------------------------------------------------------------------------------------------

#[cargo_test]
#[ignore = "dev-dependency cycles produce a cyclic crate graph"]
fn dev_dependency_cycles_do_not_create_cycles_in_the_crate_graph() {
    // a --dev--> b --> a
    //
    // Cargo allows this, and it's common in workspaces. rust-analyzer drops the dependency that
    // closes a cycle, and which one that is depends on the order of the crates, so the dev
    // dependency has to be the one that is left out.
    let a = lib("a").manifest("[dev-dependencies]\nb = { path = \"../b\" }");
    let b = lib("b").path_dependency(&a);
    let w = workspace_of([a, b]);

    // This also validates that the crate graph has no cycles
    let output = w.discover("a");

    output.assert_packages(["a", "b"]);
    output.assert_deps("b", ["a"]);
    output.assert_deps("a", []);
}

#[cargo_test]
#[ignore = "builds of compile-time dependencies that fail are not reported to the user"]
fn failing_builds_of_compile_time_dependencies_are_reported_as_progress() {
    let w = workspace_with_failing_builds();

    let output = w.discover("app");

    // The progress messages are the only thing that rust-analyzer shows the user while discovery
    // is running, so they are how they find out why `OUT_DIR` or a proc macro is missing
    let progress = output.progress_messages();
    let reported = |package: &str, kind: &str| {
        progress
            .iter()
            .any(|m| m.contains(package) && m.contains(kind) && m.contains("fail"))
    };
    assert!(reported("app", "build script"), "{progress:?}");
    assert!(reported("pm", "proc"), "{progress:?}");

    // Nothing is reported about what built fine
    assert!(!reported("healthy", ""), "{progress:?}");
}

#[cargo_test]
#[ignore = "the order of the crates and cfgs is random"]
fn output_is_deterministic() {
    // A workspace with many crates and features, so that it's very unlikely for the order of
    // them to be the same by chance
    let mut app = lib("app")
        .feature("one", [] as [&str; 0])
        .feature("two", [] as [&str; 0])
        .feature("three", [] as [&str; 0])
        .feature("four", [] as [&str; 0]);
    let mut members = vec![];
    for i in 0..8 {
        let member = lib(format!("member{i}"));
        app = app.path_dependency(&member);
        members.push(member);
    }
    members.push(app);
    let w = workspace_of(members);

    let outputs: Vec<_> = (0..3)
        .map(|_| w.discover_with_flag("app", "--all-features"))
        .collect();

    // Rust-analyzer reloads the project every time that it changes, so running discover again
    // when nothing has changed shouldn't cause that
    assert_eq!(outputs[0].raw(), outputs[1].raw());
    assert_eq!(outputs[1].raw(), outputs[2].raw());
}

#[cargo_test]
#[ignore = "feature flags are not applied to the build of proc macros and build scripts"]
fn features_apply_to_proc_macros_and_build_scripts() {
    // app has an optional dependency on a proc macro, and a build script that reacts to the
    // feature that enables it
    let pm = lib("pm").proc_macro();
    let app = lib("app")
        .raw_dependency("pm", r#"{ path = "../pm", optional = true }"#)
        .feature("extra", ["dep:pm"])
        .file(
            "build.rs",
            r#"
                fn main() {
                    if std::env::var_os("CARGO_FEATURE_EXTRA").is_some() {
                        println!("cargo:rustc-env=EXTRA_ON=yes");
                    }
                }
            "#,
        );
    let w = workspace_of([app, pm]);

    let output = w.discover_with_flag("app", "--all-features");

    // The feature is on in the reported cfgs, so the proc macro that it enables has to have been
    // built, and the build script has to have seen the feature
    let (_, app) = output.find_with_root_module("app", "src/lib.rs");
    assert!(
        app["cfg"]
            .as_array()
            .unwrap()
            .contains(&json!("feature=\"extra\""))
    );
    assert_eq!(app["env"]["EXTRA_ON"], "yes");

    let (_, pm) = output.find("pm");
    assert!(
        pm["proc_macro_dylib_path"].is_string(),
        "no dylib for the proc macro: {pm}"
    );
}

/// A workspace with an `app` crate that has a build script, and a normal dependency, a dev
/// dependency, and a build dependency
fn workspace_with_every_kind_of_dependency() -> utils::Workspace {
    let normal = lib("normal");
    let dev = lib("dev");
    let build = lib("build");
    let app = lib("app")
        .path_dependency(&normal)
        .manifest(
            "[dev-dependencies]\ndev = { path = \"../dev\" }\n\n\
             [build-dependencies]\nbuild = { path = \"../build\" }",
        )
        .file("build.rs", "fn main() {}");

    workspace_of([app, normal, dev, build])
}

#[cargo_test]
#[ignore = "lib targets depend on the build dependencies of their package"]
fn lib_target_does_not_depend_on_build_dependencies() {
    let w = workspace_with_every_kind_of_dependency();

    let output = w.discover("app");

    // Build dependencies can only be used by the build script, and rust-analyzer would resolve
    // them in the lib (and let them clash with other dependencies that have the same name)
    let (_, lib) = output.find_with_root_module("app", "src/lib.rs");
    let dep_names = output.dep_names(lib);
    assert!(dep_names.contains(&"normal".to_string()), "{dep_names:?}");
    assert!(!dep_names.contains(&"build".to_string()), "{dep_names:?}");
}

#[cargo_test]
#[ignore = "build scripts depend on everything that their package does"]
fn build_script_only_depends_on_build_dependencies() {
    let w = workspace_with_every_kind_of_dependency();

    let output = w.discover("app");

    // The build script is compiled before the lib, so it can't use it either
    let (_, build_script) = output.find_with_root_module("app", "build.rs");
    assert_eq!(output.dep_names(build_script), ["build"]);
}

#[cargo_test]
fn non_workspace_member_keeps_crates_for_its_bins_and_build_scripts() {
    // app --> standalone, which is a path dependency that is excluded from the workspace and
    // therefore not a member of it
    let standalone = lib("standalone")
        .file("src/main.rs", "fn main() {}")
        .file("build.rs", "fn main() {}")
        .file("tests/integration.rs", "")
        .file("examples/demo.rs", "fn main() {}")
        .file("benches/speed.rs", "");
    let app = lib("app").path_dependency(&standalone);
    let w = workspace().member(app).excluded(standalone).build();

    let output = w.discover("app");

    let standalone: Vec<_> = output
        .crates()
        .iter()
        .filter(|c| c["display_name"] == "standalone")
        .collect();

    // Its lib, bin, and build script all have a crate. Nothing can depend on the bin and the build
    // script, but without a crate their files don't belong to any crate. When one of them is
    // opened, rust-analyzer finds that out and runs discover again for the file, so keeping the
    // crates is what lets them be analyzed right away.
    let mut root_modules: Vec<_> = standalone
        .iter()
        .map(|c| {
            Path::new(c["root_module"].as_str().unwrap())
                .strip_prefix(w.file("standalone"))
                .unwrap()
                .to_path_buf()
        })
        .collect();
    root_modules.sort();
    assert_eq!(
        root_modules,
        [
            Path::new("build.rs"),
            Path::new("src/lib.rs"),
            Path::new("src/main.rs"),
        ]
    );

    // None of them are members, so rust-analyzer treats their sources as immutable libraries
    assert!(standalone.iter().all(|c| c["is_workspace_member"] == false));

    // The tests, examples, and benches of packages that aren't members are left out, because
    // cargo doesn't resolve the dev-dependencies that they could use
    for dir in ["tests", "examples", "benches"] {
        assert!(
            !standalone.iter().any(|c| {
                Path::new(c["root_module"].as_str().unwrap())
                    .starts_with(w.file("standalone").join(dir))
            }),
            "unexpected crate for {dir}"
        );
    }
}

#[cargo_test]
#[ignore = "excluded directories are relative, so they are resolved against the wrong directory"]
fn excluded_directories_are_absolute() {
    // root_pkg, which is the root package of the workspace, and app, which depends on it.
    // Relative directories are resolved against the directory of the build file, which for
    // discovery on app is not the one of root_pkg.
    let root_pkg = lib("root_pkg")
        .manifest("[workspace]\nmembers = [\"app\"]")
        .file(
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [dependencies]\nroot_pkg = { path = \"..\" }\n",
        )
        .file("app/src/lib.rs", "")
        .build();

    let output = discover_json(&root_pkg, root_pkg.file("app/Cargo.toml"));

    for c in output.crates() {
        for dir in c["source"]["exclude_dirs"].as_array().unwrap() {
            assert!(
                Path::new(dir.as_str().unwrap()).is_absolute(),
                "relative excluded directory {dir} in {}",
                c["display_name"]
            );
        }
    }

    // The target directory of the workspace is inside the source of its root package
    let (_, root_pkg_crate) = output.find("root_pkg");
    assert!(
        root_pkg_crate["source"]["exclude_dirs"]
            .as_array()
            .unwrap()
            .contains(&json!(root_pkg.file("target").to_str().unwrap())),
        "the target directory is not excluded: {}",
        root_pkg_crate["source"]
    );
}

#[cargo_test]
#[ignore = "discover fails when the log file can't be created"]
fn unusable_log_location_does_not_abort_discover() {
    let foo = lib("foo").build();

    // A file where the directory for the logs should be
    let not_a_directory = foo.file("not-a-directory");
    fs::write(&not_a_directory, "").unwrap();

    // The log location is a flag of the binary itself, so it goes before the subcommand. A
    // problem with the logs shouldn't prevent rust-analyzer from getting its project.
    let output = DiscoverOutput::run(
        foo.subspace()
            .arg("--log-location")
            .arg(&not_a_directory)
            .arg("discover")
            .arg(json!({ "buildfile": foo.file("Cargo.toml") }).to_string()),
    );

    assert!(output.contains("foo"));
}
