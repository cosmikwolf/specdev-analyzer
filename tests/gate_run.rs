//! The gate runner: `specdev-analyzer gate --label <l> [--module <m>]…`.
//!
//! Binary-level, because every property under test is a property of the
//! process: the working directory the steps run in, stdin being closed,
//! which stream the captured output lands on, the exit code, and what is
//! left in `gen-lineage.toml` afterwards.

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

/// `verified` for one module, read out of the `gen-plan` report.
fn verified(root: &Path, module: &str) -> bool {
    let (code, out, err) = run(root, &["report", "gen-plan"]);
    assert_eq!(code, 0, "{err}");
    let report: serde_yaml::Value = serde_yaml::from_str(&out).unwrap();
    report["modules"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|m| m["module"].as_str() == Some(module))
        .unwrap_or_else(|| panic!("no row for {module} in {out}"))["verified"]
        .as_bool()
        .unwrap()
}

#[test]
fn a_passing_gate_writes_a_batch_row_and_one_row_per_module() {
    let root = tmp("pass");
    project(&root, ONE_GATE);
    let (code, out, err) = run(
        &root,
        &[
            "gate", "--label", "ok", "--module", "Widget", "--module", "Gadget",
        ],
    );
    assert_eq!(code, 0, "{out}{err}");
    // Three rows: the batch row every run writes, plus one per module.
    assert!(out.contains("gate ok pass batch key "), "{out}");
    assert!(out.contains("gate ok pass module Widget key "), "{out}");
    assert!(out.contains("gate ok pass module Gadget key "), "{out}");
    // A step's stderr on a pass is surfaced — the compiler warnings a
    // human reader used to see are not lost — and on stderr, not stdout.
    assert!(err.contains("warn-stderr"), "{err}");
    assert!(!out.contains("warn-stderr"), "{out}");
    // And both modules read verified.
    assert!(verified(&root, "Widget"));
    assert!(verified(&root, "Gadget"));
}

#[test]
fn a_failing_step_stops_the_gate_prints_both_streams_and_writes_fail_rows() {
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
    // It ran and failed, so it records: a cheap re-gate is the safe way.
    assert!(out.contains("gate bad fail batch key "), "{out}");
    assert!(out.contains("gate bad fail module Widget key "), "{out}");
}

#[test]
fn a_step_that_cannot_start_records_nothing() {
    let root = tmp("nostart");
    project(&root, GATES);
    // A pass first, so there is a row that must survive.
    let (code, _, _) = run(&root, &["gate", "--label", "ok", "--module", "Widget"]);
    assert_eq!(code, 0);
    let before = lineage(&root);

    let (code, out, err) = run(&root, &["gate", "--label", "nostart", "--module", "Widget"]);
    assert_eq!(code, 2, "the gate did not run: {out}{err}");
    assert!(err.contains("could not start"), "{err}");
    assert!(err.contains("Nothing was recorded"), "{err}");
    assert_eq!(
        lineage(&root),
        before,
        "no row: the gate did not run, so the standing rows are still true"
    );
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
fn a_token_resolves_against_architecture_toml_and_a_missing_field_records_nothing() {
    let root = tmp("token");
    project(
        &root,
        "gates:\n  - label: t\n    scope: module\n    steps: [[/bin/echo, \"${root.platform.target_triple}\"]]\n",
    );
    // No architecture.toml at all: exit 2, no row.
    let (code, _, err) = run(&root, &["gate", "--label", "t", "--module", "Widget"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("architecture.toml"), "{err}");
    assert_eq!(lineage(&root), "", "a token error writes no row");

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
    assert_eq!(lineage(&root), "");

    fs::write(
        root.join("specs/architecture.toml"),
        "[platform]\ntarget_triple = \"thumbv7em-none-eabihf\"\n",
    )
    .unwrap();
    let (code, out, err) = run(&root, &["gate", "--label", "t", "--module", "Widget"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.contains("gate t pass module Widget"), "{out}");
}

#[test]
fn scope_decides_what_module_means() {
    let root = tmp("scope");
    project(&root, GATES);
    // A module-scope gate with no `--module` would write only a batch row
    // and leave every module unverified. Refused.
    let (code, _, err) = run(&root, &["gate", "--label", "ok"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("needs at least one `--module`"), "{err}");
    assert_eq!(lineage(&root), "");

    // A batch gate ignores `--module`, and says so.
    let (code, out, err) = run(&root, &["gate", "--label", "build", "--module", "Widget"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(err.contains("ignoring 1 `--module`"), "{err}");
    assert!(out.contains("gate build pass batch key "), "{out}");
    assert!(!out.contains("module Widget"), "no module row: {out}");
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

#[test]
fn rows_under_an_undeclared_label_are_pruned_and_printed() {
    let root = tmp("prune");
    project(&root, GATES);
    run(&root, &["gate", "--label", "ok", "--module", "Widget"]);
    run(&root, &["gate", "--label", "build"]);
    assert!(lineage(&root).contains("label = \"build\""));

    // Rename `ok` to `fast` and drop `build`: the old rows are dead
    // weight in committed lineage — `gen-plan` already ignores them.
    fs::write(
        root.join("specs/specdev/profile.yaml"),
        format!("{HEAD}gates:\n  - label: fast\n    scope: module\n    steps: [[/bin/echo, hi]]\n"),
    )
    .unwrap();
    let (code, out, err) = run(&root, &["gate", "--label", "fast", "--module", "Widget"]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(
        out.contains("pruned ok module Widget (label undeclared)"),
        "{out}"
    );
    assert!(out.contains("pruned ok batch (label undeclared)"), "{out}");
    assert!(
        out.contains("pruned build batch (label undeclared)"),
        "{out}"
    );
    let after = lineage(&root);
    assert!(!after.contains("\"ok\""), "{after}");
    assert!(!after.contains("\"build\""), "{after}");
    assert!(after.contains("\"fast\""), "{after}");
}

#[test]
fn a_template_edit_flips_verified_to_false() {
    // The unit's exit criterion. `input_hash` hashes the assembled module
    // prompt, which carries no shim template: without `profile_hash` in
    // the key a template edit rewrites every derived file while every
    // module stays `unchanged` and `verified`, and nothing re-gates.
    let root = tmp("template");
    project(&root, ONE_GATE);
    fs::create_dir_all(root.join("specs/specdev/templates/nested")).unwrap();
    fs::write(
        root.join("specs/specdev/templates/nested/shim.ts.tmpl"),
        "export { default } from '{source_relative}';\n",
    )
    .unwrap();

    let (code, _, err) = run(&root, &["gate", "--label", "ok", "--module", "Widget"]);
    assert_eq!(code, 0, "{err}");
    assert!(verified(&root, "Widget"));

    // A file one directory deep: bootstrap copies templates flat today,
    // and the hash must not assume it.
    fs::write(
        root.join("specs/specdev/templates/nested/shim.ts.tmpl"),
        "export * from '{source_relative}';\n",
    )
    .unwrap();
    assert!(
        !verified(&root, "Widget"),
        "a template edit must re-key the gate"
    );

    // Re-gating against the edited template restores it.
    let (code, _, _) = run(&root, &["gate", "--label", "ok", "--module", "Widget"]);
    assert_eq!(code, 0);
    assert!(verified(&root, "Widget"));
}

#[test]
fn a_paired_files_target_edit_flips_verified_to_false() {
    // `tsconfig.json` is externally owned after its first write, so a
    // `strict: false` flip is invisible to every other hash — and it
    // changes what the gate proves.
    let root = tmp("paired");
    project(&root, ONE_GATE);
    fs::create_dir_all(root.join("specs/specdev/templates")).unwrap();
    fs::write(root.join("specs/specdev/templates/tsconfig.json"), "{}\n").unwrap();
    fs::write(
        root.join("specs/specdev/profile.yaml"),
        format!(
            "{HEAD}paired_files:\n  - template_file: templates/tsconfig.json\n    target: tsconfig.json\n{ONE_GATE}"
        ),
    )
    .unwrap();
    fs::write(root.join("tsconfig.json"), "{\"strict\": true}\n").unwrap();

    let (code, _, err) = run(&root, &["gate", "--label", "ok", "--module", "Widget"]);
    assert_eq!(code, 0, "{err}");
    assert!(verified(&root, "Widget"));

    fs::write(root.join("tsconfig.json"), "{\"strict\": false}\n").unwrap();
    assert!(!verified(&root, "Widget"));
}

#[test]
fn the_key_is_hashed_after_the_run_not_before() {
    // The rule that keeps a row usable, now with a second moving part.
    // A gate rewrites files
    // that are in its own key: an install step rewrites the lockfile, and
    // a first `next build` rewrites `tsconfig.json`, which is a paired
    // target. Hash before the run and the row is stale the instant it is
    // written — the module would re-gate on every single run, forever.
    let root = tmp("hash-after");
    project(&root, ONE_GATE);
    fs::create_dir_all(root.join("specs/specdev/templates")).unwrap();
    fs::write(root.join("specs/specdev/templates/tsconfig.json"), "{}\n").unwrap();
    fs::write(
        root.join("specs/specdev/profile.yaml"),
        format!(
            "{HEAD}paired_files:\n  - template_file: templates/tsconfig.json\n    target: tsconfig.json\n\
             gates:\n  - label: ok\n    scope: module\n    steps:\n      \
             - [/bin/sh, -c, \"echo written-by-the-gate > tsconfig.json; echo lock > Cargo.lock\"]\n"
        ),
    )
    .unwrap();
    fs::write(root.join("tsconfig.json"), "before\n").unwrap();

    let (code, out, err) = run(&root, &["gate", "--label", "ok", "--module", "Widget"]);
    assert_eq!(code, 0, "{out}{err}");
    // The gate rewrote both halves of its own key while it ran. The row
    // it wrote is keyed on what it left behind, so it reads current.
    assert!(
        verified(&root, "Widget"),
        "the row must key on the post-run tree, or it is stale on arrival"
    );
    assert_eq!(
        fs::read_to_string(root.join("tsconfig.json")).unwrap(),
        "written-by-the-gate\n"
    );
}

#[test]
fn gate_refuses_a_project_with_no_module_scope_gate() {
    // The precondition codegen enforces, at the other entrance: a batch
    // gate can still be run by hand, but a registry that can never make
    // any module `verified` is refused wherever it is met.
    let root = tmp("nomodulegate");
    project(
        &root,
        "gates:\n  - label: build\n    scope: batch\n    steps: [[/bin/echo, x]]\n",
    );
    let (code, out, err) = run(&root, &["gate", "--label", "build"]);
    assert_eq!(code, 2, "{out}{err}");
    assert!(err.contains("none with `scope: module`"), "{err}");
    assert!(err.starts_with("gate:"), "the caller names itself: {err}");
    assert_eq!(lineage(&root), "");
}
