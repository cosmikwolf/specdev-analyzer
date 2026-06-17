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

use specdev_analyzer::{extract, report::Report, validate};

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

#[test]
fn extract_end_to_end_validates_and_emits_canonical_yaml() {
    let project = fixture("extract-end-to-end");
    let result = extract::run_project(&project, None).expect("extract should succeed");
    // Module list is populated.
    assert_eq!(result.modules.len(), 1, "expected one module (Blinky)");
    assert_eq!(result.modules[0].name, "Blinky");
    // Architecture has the executor's output: two embassy.Task entities
    // (one per task entry in the module TOML), one Module entity.
    let entities = result
        .architecture
        .get("entities")
        .and_then(|v| v.as_array())
        .expect("architecture.entities");
    assert_eq!(entities.len(), 3, "expected 1 Module + 2 embassy.Task");
    let kinds: std::collections::BTreeSet<String> = entities
        .iter()
        .filter_map(|e| e.get("kind").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    assert!(kinds.contains("Module"));
    assert!(kinds.contains("embassy.Task"));
}

#[test]
fn extract_round_trip_is_byte_identical() {
    use std::io::Read;

    let project = fixture("extract-end-to-end");
    let out_dir =
        std::env::temp_dir().join(format!("specdev-reduction-extract-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_dir);

    let r1 = extract::run_project(&project, None).unwrap();
    extract::write(&r1, &out_dir).unwrap();
    let arch_first = std::fs::read_to_string(out_dir.join("architecture.yaml")).unwrap();

    let r2 = extract::run_project(&project, None).unwrap();
    let out_dir_2 =
        std::env::temp_dir().join(format!("specdev-reduction-extract-2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out_dir_2);
    extract::write(&r2, &out_dir_2).unwrap();
    let arch_second = std::fs::read_to_string(out_dir_2.join("architecture.yaml")).unwrap();

    assert_eq!(
        arch_first, arch_second,
        "extract output must be byte-identical across runs"
    );

    // Extra paranoia: bytes match.
    let mut a = Vec::new();
    std::fs::File::open(out_dir.join("architecture.yaml"))
        .unwrap()
        .read_to_end(&mut a)
        .unwrap();
    let mut b = Vec::new();
    std::fs::File::open(out_dir_2.join("architecture.yaml"))
        .unwrap()
        .read_to_end(&mut b)
        .unwrap();
    assert_eq!(a, b);

    let _ = std::fs::remove_dir_all(&out_dir);
    let _ = std::fs::remove_dir_all(&out_dir_2);
}

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
