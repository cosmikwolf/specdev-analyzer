//! Integration tests for the validate pipeline.
//!
//! These tests exercise the analyzer against:
//!   * the `blinky-a2dp` example project (should pass cleanly)
//!   * each invalid fixture under `tests/specs/invalid/` (each should flag a
//!     specific rule)

use std::path::{Path, PathBuf};

use specdev_analyzer::validate;

fn repo_root() -> PathBuf {
    // tests/ lives directly under the crate root.
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn workspace_root() -> PathBuf {
    repo_root()
        .parent()
        .expect("analyzer crate has a parent dir")
        .to_path_buf()
}

fn schema_dir() -> PathBuf {
    workspace_root().join("specdev/schema/firmware/v1")
}

fn domain_file() -> PathBuf {
    workspace_root().join("specdev/domains/firmware/domain.toml")
}

fn blinky_root() -> PathBuf {
    workspace_root().join("specdev/examples/blinky-a2dp")
}

fn multi_pkg_root() -> PathBuf {
    repo_root().join("tests/specs/multi-pkg")
}

#[test]
fn multi_pkg_validates_cleanly() {
    let inputs =
        validate::resolve_inputs(&multi_pkg_root(), Some(&domain_file())).expect("resolve inputs");
    let report = validate::run(&inputs).expect("validate");
    let findings = report.findings();
    assert!(
        findings.is_empty(),
        "expected no findings on multi-pkg; got:\n{}",
        report.render_plain()
    );
}

#[test]
fn blinky_a2dp_passes_cleanly() {
    let inputs =
        validate::resolve_inputs(&blinky_root(), Some(&domain_file())).expect("resolve inputs");
    let report = validate::run(&inputs).expect("validate");
    let findings = report.findings();
    assert!(
        findings.is_empty(),
        "expected no findings on blinky-a2dp; got:\n{}",
        report.render_plain()
    );
}

#[test]
fn invalid_metadata_typo_flagged() {
    let fixture = repo_root().join("tests/specs/invalid/metadata_typo.toml");
    let report = validate::validate_standalone_module(&fixture, &schema_dir()).expect("validate");
    let rendered = report.render_plain();
    assert!(
        rendered.contains("ConcurrenceModel")
            || rendered.to_lowercase().contains("additionalproperties")
            || rendered.to_lowercase().contains("unevaluated")
            || rendered.to_lowercase().contains("not allowed"),
        "expected unknown-metadata-key error; got:\n{rendered}"
    );
}

#[test]
fn invalid_bad_layer_flagged() {
    let fixture = repo_root().join("tests/specs/invalid/bad_layer.toml");
    let report = validate::validate_standalone_module(&fixture, &schema_dir()).expect("validate");
    let rendered = report.render_plain();
    assert!(
        rendered.contains("drivers") || rendered.to_lowercase().contains("enum"),
        "expected layer enum error; got:\n{rendered}"
    );
}

#[test]
fn invalid_fsm_missing_event_flagged() {
    let fixture = repo_root().join("tests/specs/invalid/fsm_missing_event.toml");
    let report = validate::validate_standalone_module(&fixture, &schema_dir()).expect("validate");
    let rendered = report.render_plain();
    assert!(
        rendered.contains("event") || rendered.to_lowercase().contains("required"),
        "expected missing-required-event error; got:\n{rendered}"
    );
}
