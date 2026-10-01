//! Test items + test-aware validation.
//!
//! Coverage:
//!   * A `[[items]]` entry with `test = true` + `verifies` against a
//!     `nature = "test"` requirement validates clean.
//!   * (No longer here: `test_requirements_have_verifiers`. The 2.0
//!     templates drop it — `check`'s `requirements_have_tests` covers a
//!     `nature = "test"` requirement, and `migrate` moves every test
//!     item's `verifies` to `requirement.verified_by`.)
//!   * The module-orphan check (`requirements_have_satisfiers`) is
//!     unaffected — `nature = "test"` requirements remain exempt there.
//!   * `verifies` naming an unknown requirement surfaces REDUCE_UNRESOLVED.
//!   * Schema mutual-exclusion: `test = true` + `satisfies` rejected;
//!     non-test item carrying `verifies` rejected; `test = true` on a
//!     non-fn kind rejected.

#[path = "common/ledger.rs"]
mod ledger;

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
        "specdev-test-items-{tag}-{}-{}",
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

fn write(target: &Path, rel: &str, body: &str) {
    std::fs::write(target.join(rel), body).unwrap();
}

fn findings_for(report: &Report, rule: &str, sev: Severity) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && f.severity == sev)
        .cloned()
        .collect()
}

fn errors(report: &Report) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

const REQS_MODULE_AND_TEST: &str = r#"[[requirement]]
id = "REQ-API-001"
text = "Implements Debug for the public value type."
nature = "module"

[[requirement]]
id = "REQ-TEST-001"
text = "Round-trip serde tests run on every PR."
nature = "test"
"#;

const MODULE_WITH_TEST_ITEM: &str = r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
name = "value"
kind = "struct"
description = "The public value type."
satisfies = ["REQ-API-001"]

[[items]]
name = "roundtrip_serde"
kind = "fn"
description = "Round-trips a Value through serde_json and asserts equality."
test = true
test_kind = "integration"
verifies = ["REQ-TEST-001"]
"#;

#[test]
fn verified_test_requirement_validates_clean() {
    let (target, _) = fresh_library("clean");
    ledger::write(&target, "requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", MODULE_WITH_TEST_ITEM);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected no errors; got:\n{:#?}",
        errors(&report)
    );
    let warns = findings_for(
        &report,
        "test_requirements_have_verifiers",
        Severity::Warning,
    );
    assert!(
        warns.is_empty(),
        "the test req is verified — no warning expected; got:\n{warns:#?}"
    );
}

#[test]
fn verifies_unknown_requirement_unresolves() {
    let module = r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
name = "roundtrip_serde"
kind = "fn"
description = "Round-trips a Value through serde_json."
test = true
test_kind = "unit"
verifies = ["REQ-NOPE-999"]
"#;
    let (target, _) = fresh_library("unresolved");
    ledger::write(&target, "requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    let unresolved = findings_for(&report, "REDUCE_UNRESOLVED", Severity::Error);
    assert_eq!(
        unresolved.len(),
        1,
        "verifies → unknown requirement must surface one REDUCE_UNRESOLVED; got:\n{:#?}",
        report.findings()
    );
    assert!(
        unresolved[0].message.contains("REQ-NOPE-999"),
        "names the bad reference: {}",
        unresolved[0].message
    );
}

#[test]
fn schema_rejects_test_item_with_satisfies() {
    let module = r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
name = "roundtrip_serde"
kind = "fn"
description = "Test item that wrongly carries satisfies."
test = true
test_kind = "unit"
verifies = ["REQ-TEST-001"]
satisfies = ["REQ-API-001"]
"#;
    let (target, _) = fresh_library("test-and-satisfies");
    ledger::write(&target, "requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        !errors(&report).is_empty(),
        "test = true with satisfies must fail schema validation (mutual exclusion)"
    );
}

#[test]
fn schema_rejects_non_test_item_with_verifies() {
    let module = r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
name = "value"
kind = "struct"
description = "Non-test item wrongly carrying verifies."
satisfies = ["REQ-API-001"]
verifies = ["REQ-TEST-001"]
"#;
    let (target, _) = fresh_library("nontest-verifies");
    ledger::write(&target, "requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        !errors(&report).is_empty(),
        "a non-test item carrying verifies must fail schema validation"
    );
}

#[test]
fn schema_rejects_test_on_non_fn_kind() {
    let module = r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
name = "value"
kind = "struct"
description = "A struct cannot be a test."
test = true
test_kind = "unit"
verifies = ["REQ-TEST-001"]
"#;
    let (target, _) = fresh_library("test-non-fn");
    ledger::write(&target, "requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        !errors(&report).is_empty(),
        "test = true on kind = struct must fail schema validation (only fns can be tests)"
    );
}
