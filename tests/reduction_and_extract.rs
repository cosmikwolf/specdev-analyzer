//! Reduction resolve + extract pipeline.
//!
//! These exercise the full validate pipeline (layout discovery → aggregator
//! compile → reductions::resolve → module schema validation → domain.toml
//! graph-check resolution + execution) and the extract pipeline against
//! on-disk fixtures.
//!
//! Resolver-internals tests live next to the resolver in
//! `src/reductions.rs`; this file's job is the wiring.

use std::path::{Path, PathBuf};

use specdev_analyzer::{report::Report, validate};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("reduction_cases")
}

fn fixture(name: &str) -> PathBuf {
    fixtures_root().join(name)
}

fn render_findings(report: &Report) -> String {
    let mut lines: Vec<String> = report
        .findings()
        .iter()
        .map(|f| format!("{:?} {}", f.severity, f.rule))
        .collect();
    lines.sort();
    lines.dedup();
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Positive: extract end-to-end produces canonical YAML deterministically.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Negative fixtures: each must surface its expected finding.
// ---------------------------------------------------------------------------

#[test]
fn bridge_lex_order_violation_emits_metaschema_003() {
    let report = validate::run_project(&fixture("bridge-lex-order-violation"), None)
        .expect("validate produces a Report");
    assert!(
        report.findings().iter().any(|f| f.rule == "METASCHEMA-003"),
        "expected METASCHEMA-003; got: {}",
        report.render_plain()
    );
}

#[test]
fn missing_extends_on_entity_emits_metaschema_001() {
    let report = validate::run_project(&fixture("missing-extends-on-entity"), None)
        .expect("validate produces a Report");
    assert!(
        report.findings().iter().any(|f| f.rule == "METASCHEMA-001"),
        "expected METASCHEMA-001; got: {}",
        report.render_plain()
    );
}

#[test]
fn unresolved_extends_emits_metaschema_002() {
    let report = validate::run_project(&fixture("unresolved-extends"), None)
        .expect("validate produces a Report");
    assert!(
        report.findings().iter().any(|f| f.rule == "METASCHEMA-002"),
        "expected METASCHEMA-002; got: {}",
        report.render_plain()
    );
}

#[test]
fn domain_toml_unresolved_kind_ref_emits_domain_003() {
    let report = validate::run_project(&fixture("domain-toml-unresolved-kind-ref"), None)
        .expect("validate produces a Report");
    assert!(
        report.findings().iter().any(|f| f.rule == "DOMAIN-003"),
        "expected DOMAIN-003; got: {}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// Snapshot: rule-code shape of the end-to-end fixture, for stable review.
// ---------------------------------------------------------------------------

#[test]
fn snapshot_extract_end_to_end_rule_codes() {
    let report = validate::run_project(&fixture("extract-end-to-end"), None).unwrap();
    insta::assert_snapshot!(render_findings(&report));
}
