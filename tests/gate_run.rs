//! The gate runner: `specdev-analyzer gate --label <l> [--module <m>]…`.
//!
//! Binary-level, because every property under test is a property of the
//! process: the working directory the steps run in, stdin being closed,
//! which stream the captured output lands on, and the exit code. A gate
//! records nothing: its result is its exit code.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir()
        .join("specdev-gate-run-it")
        .join(format!("{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

const HEAD: &str = "name: test-profile\nlanguage: rust\n";

/// One module-scope gate. `verified` needs a current pass row from
/// *every* declared module-scope gate, so a fixture that declares three
/// of them can never read `verified: true` off one run — that is unit
/// 3's rule, and it is held by `tests/gen_lineage.rs`, not here.
const ONE_GATE: &str = r#"gates:
  - label: ok
    scope: module
    steps:
      - [/bin/echo, hello-stdout]
      - [/bin/sh, -c, "echo warn-stderr 1>&2"]
"#;

/// Registry used by most cases: a passing module gate, a failing one, a
/// batch gate, and one whose executable does not exist.
const GATES: &str = r#"gates:
  - label: ok
    scope: module
    steps:
      - [/bin/echo, hello-stdout]
      - [/bin/sh, -c, "echo warn-stderr 1>&2"]
  - label: bad
    scope: module
    steps:
      - [/bin/sh, -c, "echo out; echo err 1>&2; exit 3"]
      - [/bin/sh, -c, "touch second-step-ran"]
  - label: build
    scope: batch
    steps: [[/bin/echo, built]]
  - label: nostart
    scope: module
    steps: [[definitely-not-a-real-binary-xyz, --go]]
"#;

/// A project with two modules and an inlined profile carrying `gates`.
fn project(root: &Path, gates: &str) {
    fs::create_dir_all(root.join("specs/specdev")).unwrap();
    fs::create_dir_all(root.join("specs/modules")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("spec.toml"),
        "spec_version = \"1.0\"\n\n[project]\nname = \"gatefixture\"\nprofile = \"test-profile\"\nlanguage = \"rust\"\n\n[paths]\nspecs = \"specs\"\nsource = \"src\"\n",
    )
    .unwrap();
    for name in ["Widget", "Gadget"] {
        fs::write(
            root.join(format!("specs/modules/{name}.toml")),
            format!("name = \"{name}\"\nlayer = \"driver\"\n"),
        )
        .unwrap();
    }
    fs::write(
        root.join("specs/specdev/profile.yaml"),
        format!("{HEAD}{gates}"),
    )
    .unwrap();
}

fn run(project: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(args)
        .arg("--project")
        .arg(project)
        .env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .output()
        .expect("spawn specdev-analyzer");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn lineage(root: &Path) -> String {
    fs::read_to_string(root.join("specs/specdev/gen-lineage.toml")).unwrap_or_default()
}

#[test]
fn a_passing_gate_prints_pass_and_records_nothing() {
    let root = tmp("pass");
    project(&root, ONE_GATE);
    let (code, out, err) = run(
        &root,
        &[
            "gate", "--label", "ok", "--module", "Widget", "--module", "Gadget",
        ],
    );
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out.trim(), "gate ok pass", "{out}");
    // A step's stderr on a pass is surfaced — the compiler warnings a
    // human reader used to see are not lost — and on stderr, not stdout.
    assert!(err.contains("warn-stderr"), "{err}");
    assert!(!out.contains("warn-stderr"), "{out}");
    assert!(!root.join("specs/specdev/gen-lineage.toml").exists(), "a gate records nothing");
}

#[test]
fn a_failing_step_stops_the_gate_and_prints_both_streams() {
    let root = tmp("fail");
    project(&root, GATES);
    let (code, out, err) = run(&root, &["gate", "--label", "bad", "--module", "Widget"]);
    // The gate's own exit code, not a fixed 1.
    assert_eq!(code, 3, "{out}{err}");
    assert!(err.contains("step 1 of 2 failed (exit 3)"), "{err}");
    // Captured output of the failing step, both streams, on our stderr.
    assert!(err.contains("out"), "{err}");
    assert!(err.contains("err"), "{err}");
    // The first non-zero exit stops the gate: step 2 never ran.
    assert!(
        !root.join("second-step-ran").exists(),
        "later steps must not run"
    );
    assert_eq!(out.trim(), "gate bad fail", "{out}");
}

#[test]
fn a_step_that_cannot_start_is_exit_2() {
    let root = tmp("nostart");
    project(&root, GATES);
    let (code, out, err) = run(&root, &["gate", "--label", "nostart", "--module", "Widget"]);
    assert_eq!(code, 2, "the gate did not run: {out}{err}");
    assert!(err.contains("could not start"), "{err}");
}

#[test]
fn steps_run_in_the_project_root_with_stdin_closed() {
    let root = tmp("cwd-stdin");
    // `spec.toml` exists only in the project root; the test process runs
    // in the analyzer repo. `read` returns non-zero at EOF — the point of
    // the case is that it terminates at all: an inherited terminal stdin
    // would wait forever.
    project(
        &root,
        "gates:\n  - label: env\n    scope: module\n    steps:\n      - [/bin/sh, -c, \"test -f spec.toml\"]\n      - [/bin/sh, -c, \"read line\"]\n",
    );
    let (code, out, err) = run(&root, &["gate", "--label", "env", "--module", "Widget"]);
    assert_eq!(code, 1, "step 1 passed in the project root: {out}{err}");
    assert!(err.contains("step 2 of 2 failed"), "{err}");
}

#[test]
fn a_token_resolves_against_architecture_toml_and_a_missing_field_is_an_error() {
    let root = tmp("token");
    project(
        &root,
        "gates:\n  - label: t\n    scope: module\n    steps: [[/bin/echo, \"${root.platform.target_triple}\"]]\n",
    );
    // No architecture.toml at all: exit 2.
    let (code, _, err) = run(&root, &["gate", "--label", "t", "--module", "Widget"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("architecture.toml"), "{err}");

    // A field that is present but empty is an error too: a token resolves
    // to a value or it fails; it never becomes an empty argument.
    fs::write(
        root.join("specs/architecture.toml"),
        "[platform]\ntarget_triple = \"\"\n",
    )
    .unwrap();
    let (code, _, err) = run(&root, &["gate", "--label", "t", "--module", "Widget"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("resolved to nothing"), "{err}");

    fs::write(
        root.join("specs/architecture.toml"),
        "[platform]\ntarget_triple = \"thumbv7em-none-eabihf\"\n",
    )
    .unwrap();
    let (code, out, err) = run(&root, &["gate", "--label", "t", "--module", "Widget"]);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out.trim(), "gate t pass", "{out}");
}

#[test]
fn scope_decides_what_module_means() {
    let root = tmp("scope");
    project(&root, GATES);
    // A module-scope gate's steps name the modules of the run.
    let (code, _, err) = run(&root, &["gate", "--label", "ok"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("needs at least one `--module`"), "{err}");

    // A batch gate ignores `--module`, and says so.
    let (code, out, err) = run(&root, &["gate", "--label", "build", "--module", "Widget"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.contains("ignoring 1 `--module`"), "{err}");
    assert_eq!(out.trim(), "gate build pass", "{out}");
}

#[test]
fn an_undeclared_label_and_an_unknown_module_are_refused() {
    let root = tmp("refuse");
    project(&root, GATES);
    let (code, _, err) = run(&root, &["gate", "--label", "nope", "--module", "Widget"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("is not declared in"), "{err}");
    assert!(
        err.contains("`ok` (module)"),
        "the message lists what is: {err}"
    );

    let (code, _, err) = run(&root, &["gate", "--label", "ok", "--module", "Nope"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no module spec under"), "{err}");
    assert_eq!(lineage(&root), "");
}
