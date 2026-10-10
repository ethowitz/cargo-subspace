//! Tests for the `cargo-subspace check` and `cargo-subspace clippy` subcommands. These tests run
//! the real binary and inspect its output

mod utils;

use cargo_test_macro::cargo_test;
use snapbox::{IntoData, str};

use crate::utils::{CargoSubspaceExt, bin, lib, workspace_of};

// TODO: check tests should run alongside cargo check with --manifest-path and check that output is
// equivalent (can't just do snapshot testing bc output could differ across cargo versions)

#[cargo_test]
fn warnings_are_reported_as_json_diagnostics() {
    let p = lib("foo")
        .file("src/lib.rs", "pub fn f() { let unused = 1; }")
        .build();

    p.check("src/lib.rs")
        .with_stdout_data(str![[r#"
...
[..]"reason":"compiler-message"[..]"message":"unused variable: `unused`"[..]
...
{"reason":"build-finished","success":true}

"#]])
        .run();
}

#[cargo_test]
fn errors_fail_the_command() {
    let p = lib("foo")
        .file(
            "src/lib.rs",
            r#"pub fn f() { let x: u32 = "not a number"; }"#,
        )
        .build();

    p.check("src/lib.rs")
        .with_status(1)
        // The failure is also reported in the format that rust-analyzer expects
        .with_stdout_data(str![[r#"
...
[..]"reason":"compiler-message"[..]"level":"error"[..]
...
{"reason":"build-finished","success":false}
{"kind":"error","error":"Failed to run check","source":null}

"#]])
        .run();
}

#[cargo_test]
fn all_targets_are_checked() {
    let p = lib("foo")
        .file("tests/integration.rs", "fn f() { let in_test = 1; }")
        .file("examples/demo.rs", "fn main() { let in_example = 1; }")
        .build();

    // Tests and examples are only built when checking all targets
    p.check("src/lib.rs")
        .with_stdout_data(
            str![[r#"
...
[..]"src_path":"[..]integration.rs"[..]"message":"unused variable: `in_test`"[..]
[..]"src_path":"[..]demo.rs"[..]"message":"unused variable: `in_example`"[..]
...
"#]]
            // The targets are checked in no particular order
            .unordered(),
        )
        .run();
}

#[cargo_test]
fn only_the_crate_containing_the_file_and_its_dependencies_are_checked() {
    // foo --> bar
    // baz
    let bar = lib("bar");
    let foo = lib("foo").path_dependency(&bar);
    let w = workspace_of([foo, bar, lib("baz")]);

    w.check("foo/src/lib.rs")
        .with_stderr_contains("[CHECKING] foo [..]")
        .with_stderr_contains("[CHECKING] bar [..]")
        .with_stderr_does_not_contain("[CHECKING] baz [..]")
        .run();

    w.check("baz/src/lib.rs")
        .with_stderr_contains("[CHECKING] baz [..]")
        .with_stderr_does_not_contain("[CHECKING] foo [..]")
        .with_stderr_does_not_contain("[CHECKING] bar [..]")
        .run();
}

#[cargo_test]
fn manifest_is_found_from_a_nested_source_file() {
    let p = lib("foo")
        .file("src/lib.rs", "pub mod outer;")
        .file("src/outer/mod.rs", "pub mod inner;")
        .file("src/outer/inner.rs", "pub fn f() { let nested = 1; }")
        .build();

    p.check("src/outer/inner.rs")
        .with_stderr_contains("[CHECKING] foo [..]")
        .with_stdout_data(str![[r#"
...
[..]"message":"unused variable: `nested`"[..]
...
"#]])
        .run();
}

#[cargo_test]
fn passthrough_args_are_forwarded_to_cargo() {
    let p = lib("foo").build();

    // `-v` makes cargo print the rustc invocations it runs
    p.check("src/lib.rs")
        .arg("--")
        .arg("-v")
        .with_stderr_contains("[RUNNING] `rustc --crate-name foo [..]")
        .run();
}

#[cargo_test]
fn clippy_reports_lints_that_check_does_not() {
    let p = bin("foo")
        .file(
            "src/main.rs",
            r#"fn main() { let x = true; if x == true { println!("hi"); } }"#,
        )
        .build();

    p.check("src/main.rs")
        .with_stdout_does_not_contain("[..]clippy::bool_comparison[..]")
        .run();

    p.clippy("src/main.rs")
        .with_stdout_data(str![[r#"
...
[..]"reason":"compiler-message"[..]clippy::bool_comparison[..]
...
"#]])
        .run();
}
