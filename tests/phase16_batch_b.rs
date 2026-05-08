//! Phase 16 — Batch B — capability tagging, `--for` filter, and two
//! new validate-time checks (`ITEMS-001` info; `CONTEXT-100` warning).
//!
//! Doc 20 §4 commits Batch B to the registry refactor plus the
//! `items_present` and `context_files_when_strict` checks. This file
//! covers behavior; the registry-shape choice (tag table vs free fns)
//! is internal and exercised by the existing test suite — the tags
//! decide which checks fire under each `--for` slice.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    report::{Finding, Report},
    validate::{self, Capability},
    Severity,
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase16-batch-b-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn fresh_library(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn findings_with_rule<'a>(report: &'a Report, rule: &str) -> Vec<&'a Finding> {
    report.findings().iter().filter(|f| f.rule == rule).collect()
}

// ===========================================================================
// 1 — items_present (ITEMS-001) info on modules with empty items[].
// ===========================================================================

#[test]
fn items_present_fires_on_empty_items_module() {
    let (target, _layout) = fresh_library("items-present-empty");
    std::fs::write(
        target.join("specs/modules/Empty.toml"),
        r#"name = "Empty"
visibility = "pub"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let hits = findings_with_rule(&report, "ITEMS-001");
    assert_eq!(
        hits.len(),
        1,
        "expected one ITEMS-001 finding on the empty module; got {hits:#?}\n{}",
        report.render_plain()
    );
    assert!(
        matches!(hits[0].severity, Severity::Info),
        "ITEMS-001 must be info-level; got {:?}",
        hits[0].severity
    );
    assert!(
        hits[0].message.contains("Empty") && hits[0].message.contains("0 items"),
        "message must name module and zero-items condition; got {}",
        hits[0].message
    );
}

#[test]
fn items_present_silent_when_module_declares_items() {
    let (target, _layout) = fresh_library("items-present-populated");
    std::fs::write(
        target.join("specs/modules/Populated.toml"),
        r#"name = "Populated"
visibility = "pub"

[[items]]
kind = "fn"
name = "noop"
description = "no op"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let hits = findings_with_rule(&report, "ITEMS-001");
    assert!(
        hits.is_empty(),
        "items_present must stay silent when items[] is non-empty; got {hits:#?}"
    );
}

// ===========================================================================
// 2 — context_files_when_strict (CONTEXT-100) warning fires whenever a
//     module has no `context_files` and the StrictGen capability is in scope.
// ===========================================================================

#[test]
fn context_files_warning_fires_in_strict_mode() {
    let (target, _layout) = fresh_library("ctx-strict");
    std::fs::write(
        target.join("specs/modules/Mech.toml"),
        r#"name = "Mech"
visibility = "pub"

[[items]]
kind = "fn"
name = "step"
description = "tick the loop"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let hits = findings_with_rule(&report, "CONTEXT-100");
    assert_eq!(
        hits.len(),
        1,
        "expected CONTEXT-100 warning on Mech without context_files in strict-leaning mode; got {hits:#?}\n{}",
        report.render_plain()
    );
    assert!(
        matches!(hits[0].severity, Severity::Warning),
        "CONTEXT-100 must be warning-level; got {:?}",
        hits[0].severity
    );
    assert!(hits[0].message.contains("Mech"));
}

#[test]
fn context_files_warning_silent_with_context_files_set() {
    let (target, _layout) = fresh_library("ctx-strict-set");
    std::fs::write(target.join("docs.md"), "facts\n").unwrap();
    std::fs::write(
        target.join("specs/modules/HasCtx.toml"),
        r#"name = "HasCtx"
visibility = "pub"
context_files = ["docs.md"]

[[items]]
kind = "fn"
name = "tick"
description = "tick"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let hits = findings_with_rule(&report, "CONTEXT-100");
    assert!(
        hits.is_empty(),
        "CONTEXT-100 must stay silent when context_files is non-empty; got {hits:#?}"
    );
}

// ===========================================================================
// 3 — `--for` filter scopes which checks fire.
// ===========================================================================

#[test]
fn for_cargo_toml_suppresses_items_and_context_warnings() {
    let (target, _layout) = fresh_library("for-cargo");
    std::fs::write(
        target.join("specs/modules/Empty.toml"),
        r#"name = "Empty"
visibility = "pub"
"#,
    )
    .unwrap();
    let report =
        validate::run_project_for(&target, None, Some(Capability::CargoToml)).unwrap();
    assert!(
        findings_with_rule(&report, "ITEMS-001").is_empty(),
        "ITEMS-001 (StrictGen-only) must not fire under --for=cargo-toml"
    );
    assert!(
        findings_with_rule(&report, "CONTEXT-100").is_empty(),
        "CONTEXT-100 (StrictGen-only) must not fire under --for=cargo-toml"
    );
}

#[test]
fn for_strict_gen_keeps_both_new_checks() {
    let (target, _layout) = fresh_library("for-strictgen");
    std::fs::write(
        target.join("specs/modules/Empty.toml"),
        r#"name = "Empty"
visibility = "pub"
"#,
    )
    .unwrap();
    let report =
        validate::run_project_for(&target, None, Some(Capability::StrictGen)).unwrap();
    assert_eq!(findings_with_rule(&report, "ITEMS-001").len(), 1);
    assert_eq!(findings_with_rule(&report, "CONTEXT-100").len(), 1);
}

// ===========================================================================
// 4 — Capability::Schema always fires regardless of filter.
// ===========================================================================

#[test]
fn schema_findings_unaffected_by_for_filter() {
    let (target, _layout) = fresh_library("for-schema");
    // Module with a schema-error: invalid visibility value (closed enum
    // pub|crate|private). Keeps `name` so the reducer can still walk
    // the module without panicking.
    std::fs::write(
        target.join("specs/modules/Bad.toml"),
        r#"name = "Bad"
visibility = "supersecret"
"#,
    )
    .unwrap();
    let report =
        validate::run_project_for(&target, None, Some(Capability::CargoToml)).unwrap();
    let schema_errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| f.rule.starts_with("SCHEMA") && matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        !schema_errs.is_empty(),
        "Schema-tagged findings must always fire even under --for=cargo-toml; got:\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 5 — CLI: unknown --for value is rejected with a clear error.
// ===========================================================================

#[test]
fn cli_for_unknown_value_rejected() {
    let (target, _layout) = fresh_library("cli-for-bad");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(["validate", "--no-color", "--for", "bogus", "--project"])
        .arg(&target)
        .output()
        .expect("running validate --for=bogus");
    assert!(!out.status.success(), "should exit non-zero on bad --for");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown capability") && stderr.contains("bogus"),
        "stderr must name the bad capability; got:\n{stderr}"
    );
}

#[test]
fn cli_for_strict_gen_runs_and_filters() {
    let (target, _layout) = fresh_library("cli-for-strictgen");
    std::fs::write(
        target.join("specs/modules/Empty.toml"),
        r#"name = "Empty"
visibility = "pub"
"#,
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(["validate", "--no-color", "--for", "strict-gen", "--project"])
        .arg(&target)
        .output()
        .expect("running validate --for=strict-gen");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ITEMS-001"),
        "strict-gen scope must include ITEMS-001 in stdout; got:\n{stdout}"
    );
    assert!(
        stdout.contains("CONTEXT-100"),
        "strict-gen scope must include CONTEXT-100; got:\n{stdout}"
    );
}
