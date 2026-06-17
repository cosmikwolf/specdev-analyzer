//! Validate pipeline against the `specs/specdev/` layout.
//!
//! Exercises four behaviors:
//!   1. Malformed `spec.toml` → file:line-precision error.
//!   2. Well-formed skeleton → layout enumeration succeeds.
//!   3. Missing-field `spec.toml` → error names the field.
//!   4. Aggregator unresolved-`$ref` and duplicate-`$id` → reported as findings.

use std::path::{Path, PathBuf};

use specdev_analyzer::{diagnostics::LocatedError, validate};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/specs/validate_cases")
        .join(name)
}

fn render_findings(report: &specdev_analyzer::Report) -> String {
    let mut rows: Vec<String> = report
        .findings()
        .iter()
        .map(|f| {
            let sev = match f.severity {
                specdev_analyzer::Severity::Error => "error",
                specdev_analyzer::Severity::Warning => "warning",
                specdev_analyzer::Severity::Info => "info",
            };
            let rule = if f.rule.is_empty() {
                "SCHEMA"
            } else {
                f.rule.as_str()
            };
            format!("{sev} {rule}")
        })
        .collect();
    rows.sort();
    rows.dedup();
    rows.join("\n")
}

#[test]
fn skeleton_minimal_validates_with_no_errors() {
    let report = validate::run_project(&fixture("skeleton-minimal"), None)
        .expect("skeleton-minimal validates");
    let (errors, warnings, _infos) = report.counts();
    assert_eq!(
        errors,
        0,
        "skeleton-minimal should not error; got: {}",
        report.render_plain()
    );
    // Expect zero warnings: every expected layout entry is present.
    assert_eq!(
        warnings,
        0,
        "skeleton-minimal should not warn; got: {}",
        report.render_plain()
    );
}

#[test]
fn skeleton_with_stray_module_validates_against_compiled_schema() {
    // The aggregator schema is compiled and each module validated against it;
    // a clean fixture with a permissive `dummy` block schema produces no SCHEMA
    // findings.
    let report = validate::run_project(&fixture("skeleton-with-stray-module"), None)
        .expect("skeleton-with-stray-module validates");
    assert!(
        !report.has_errors(),
        "stray module should not error: {}",
        report.render_plain()
    );
    let schema = report
        .findings()
        .iter()
        .filter(|f| f.rule == "SCHEMA")
        .count();
    assert_eq!(
        schema, 0,
        "permissive dummy schema should produce no SCHEMA errors"
    );
}

#[test]
fn malformed_spec_toml_errors_with_file_line_col() {
    let err = validate::run_project(&fixture("spectoml-bad-syntax"), None)
        .expect_err("malformed spec.toml must error");
    let loc = err.downcast_ref::<LocatedError>();
    assert!(loc.is_some(), "expected LocatedError; got: {err:#}");
    let loc = loc.unwrap();
    assert!(loc.line >= 3, "expected an early-line error; got: {loc}");
    assert!(
        loc.path.ends_with("spec.toml"),
        "expected error path to end with spec.toml; got {}",
        loc.path.display()
    );
}

#[test]
fn missing_profile_field_errors_with_field_name() {
    let err = validate::run_project(&fixture("spectoml-missing-profile"), None)
        .expect_err("missing profile must error");
    let s = format!("{err:#}");
    assert!(
        s.contains("project") || s.contains("profile"),
        "expected error to name [project] or profile; got: {s}"
    );
}

#[test]
fn aggregator_unresolved_ref_emits_aggregator_001_error() {
    let report = validate::run_project(&fixture("aggregator-unresolved-ref"), None)
        .expect("aggregator-unresolved-ref produces a Report");
    assert!(report.has_errors());
    assert!(
        report.findings().iter().any(|f| f.rule == "AGGREGATOR-001"),
        "expected AGGREGATOR-001 finding; got: {}",
        report.render_plain()
    );
}

#[test]
fn aggregator_duplicate_id_emits_aggregator_002_error() {
    let report = validate::run_project(&fixture("aggregator-duplicate-id"), None)
        .expect("aggregator-duplicate-id produces a Report");
    assert!(report.has_errors());
    assert!(
        report.findings().iter().any(|f| f.rule == "AGGREGATOR-002"),
        "expected AGGREGATOR-002 finding; got: {}",
        report.render_plain()
    );
}

// -----------------------------------------------------------------------------
// Snapshot the (rule, severity) pairs for the two non-error fixtures, so the
// shape of LAYOUT-/REDUCTION-/AGGREGATOR-/DOMAIN-/LAYOUT-005 output is
// reviewable on every diff. Stable fields only — paths and free-form
// messages are excluded so refactors don't churn.
// -----------------------------------------------------------------------------

#[test]
fn snapshot_skeleton_minimal_rule_codes() {
    let report = validate::run_project(&fixture("skeleton-minimal"), None).unwrap();
    insta::assert_snapshot!(render_findings(&report));
}

#[test]
fn snapshot_skeleton_with_stray_module_rule_codes() {
    let report = validate::run_project(&fixture("skeleton-with-stray-module"), None).unwrap();
    insta::assert_snapshot!(render_findings(&report));
}

#[test]
fn snapshot_aggregator_unresolved_ref_rule_codes() {
    let report = validate::run_project(&fixture("aggregator-unresolved-ref"), None).unwrap();
    insta::assert_snapshot!(render_findings(&report));
}

#[test]
fn snapshot_aggregator_duplicate_id_rule_codes() {
    let report = validate::run_project(&fixture("aggregator-duplicate-id"), None).unwrap();
    insta::assert_snapshot!(render_findings(&report));
}
