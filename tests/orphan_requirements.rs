//! Orphan-requirements check + `nature` field (F-006).
//!
//! The check fires for `nature = "module"`
//! requirements that no module's `satisfies` array references; the
//! other natures (`architecture`, `build`, `test`) are exempt — they
//! intentionally don't have a module satisfier.
//!
//! Severity is error in the 2.0 templates (set in the per-profile
//! domain.toml.template `[validation.rules]` table); the check itself
//! is wired in both `firmware-layered` and `library-api` profiles
//! out of the box.

use crate::ledger;

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    report::{Finding, Report},
    validate, Severity,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-f006-{tag}-{}-{}",
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
        &full_plugin(),
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

fn orphans(report: &Report, rule: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// 1 — Default `nature` is `"module"`. A requirement omitted from every
//     module's `satisfies` array fires `requirements_have_satisfiers`.
// ---------------------------------------------------------------------------
#[test]
fn orphan_module_requirement_fires_warning() {
    let (target, _) = fresh_library("orphan-module");
    ledger::write(&target, "requirements.toml", r#"[[requirement]]
id = "REQ-API-001"
text = "Public types must implement Debug."

[[requirement]]
id = "REQ-API-002"
text = "Public types must implement Clone where applicable."
"#);
    std::fs::write(
        target.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let warnings = orphans(&report, "requirements_have_satisfiers");
    assert_eq!(warnings.len(), 1, "exactly one orphan; got: {warnings:?}");
    let msg = &warnings[0].message;
    assert!(
        msg.contains("REQ-API-002") && msg.contains("nature = \"module\""),
        "warning names the orphan + nature: {msg}"
    );
    assert!(
        !msg.contains("REQ-API-001"),
        "REQ-API-001 IS satisfied — must not fire: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 2 — `nature = "architecture"` exempts the requirement from the check.
//     Same for `"build"` and `"test"`.
// ---------------------------------------------------------------------------
#[test]
fn architecture_build_test_natures_exempt_from_check() {
    let (target, _) = fresh_library("nature-exempt");
    ledger::write(&target, "requirements.toml", r#"[[requirement]]
id = "REQ-ARCH-001"
text = "All public types live in pub module exports."
nature = "architecture"

[[requirement]]
id = "REQ-BUILD-001"
text = "Crate compiles with --no-default-features."
nature = "build"

[[requirement]]
id = "REQ-TEST-001"
text = "Round-trip serde tests run on every PR."
nature = "test"

[[requirement]]
id = "REQ-API-001"
text = "Implements Debug."
nature = "module"
"#);
    std::fs::write(
        target.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let warnings = orphans(&report, "requirements_have_satisfiers");
    assert!(
        warnings.is_empty(),
        "non-module natures must be exempt; got: {warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// 3 — Multiple orphans: each gets its own finding (no batching).
// ---------------------------------------------------------------------------
#[test]
fn multiple_orphans_emit_one_finding_each() {
    let (target, _) = fresh_library("multi-orphan");
    ledger::write(&target, "requirements.toml", r#"[[requirement]]
id = "REQ-001"
text = "first orphan"

[[requirement]]
id = "REQ-002"
text = "second orphan"

[[requirement]]
id = "REQ-003"
text = "third orphan"
"#);
    std::fs::write(
        target.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let warnings = orphans(&report, "requirements_have_satisfiers");
    assert_eq!(warnings.len(), 3);
}

// ---------------------------------------------------------------------------
// 4 — `nature` is schema-validated as a closed enum; an unknown value
//     errors at validate-time before the graph check runs.
// ---------------------------------------------------------------------------
#[test]
fn unknown_nature_value_rejected_at_schema_time() {
    let (target, _) = fresh_library("unknown-nature");
    ledger::write(&target, "requirements.toml", r#"[[requirement]]
id = "REQ-001"
text = "bad nature"
nature = "philosophical"
"#);
    std::fs::write(
        target.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errors
            .iter()
            .any(|f| f.message.contains("nature") || f.message.contains("philosophical")),
        "expected schema error mentioning the bad value; got: {errors:?}"
    );
}

// ---------------------------------------------------------------------------
// 5 — Project with zero requirements emits no warnings (empty universe
//     is no-finding, not all-clean-edge-case-bug).
// ---------------------------------------------------------------------------
#[test]
fn empty_requirements_universe_emits_no_warnings() {
    let (target, _) = fresh_library("empty-reqs");
    ledger::write(&target, "requirements.toml", "");
    std::fs::write(
        target.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let warnings = orphans(&report, "requirements_have_satisfiers");
    assert!(
        warnings.is_empty(),
        "empty universe = no warnings; got: {warnings:?}"
    );
}

// ---------------------------------------------------------------------------
// 6 — Mixed scenario: some orphans + some satisfied + some non-module.
//     Only the module-nature orphans fire warnings.
// ---------------------------------------------------------------------------
#[test]
fn mixed_natures_only_module_orphans_fire() {
    let (target, _) = fresh_library("mixed");
    ledger::write(&target, "requirements.toml", r#"[[requirement]]
id = "REQ-COVERED-001"
text = "covered"

[[requirement]]
id = "REQ-ORPHAN-001"
text = "orphan, default nature"

[[requirement]]
id = "REQ-MODULE-002"
text = "explicit module orphan"
nature = "module"

[[requirement]]
id = "REQ-ARCH-001"
text = "exempt"
nature = "architecture"

[[requirement]]
id = "REQ-BUILD-001"
text = "exempt"
nature = "build"
"#);
    std::fs::write(
        target.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-COVERED-001"]
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let warnings = orphans(&report, "requirements_have_satisfiers");
    let names: Vec<&str> = warnings.iter().map(|f| f.message.as_str()).collect();
    assert_eq!(
        warnings.len(),
        2,
        "two module-nature orphans; got {names:?}"
    );
    assert!(
        names.iter().any(|m| m.contains("REQ-ORPHAN-001")),
        "REQ-ORPHAN-001 (default nature) should fire: {names:?}"
    );
    assert!(
        names.iter().any(|m| m.contains("REQ-MODULE-002")),
        "REQ-MODULE-002 (explicit module) should fire: {names:?}"
    );
    assert!(
        !names.iter().any(|m| m.contains("REQ-ARCH-001")),
        "architecture exempt: {names:?}"
    );
    assert!(
        !names.iter().any(|m| m.contains("REQ-BUILD-001")),
        "build exempt: {names:?}"
    );
    assert!(
        !names.iter().any(|m| m.contains("REQ-COVERED-001")),
        "covered must not fire: {names:?}"
    );
}
