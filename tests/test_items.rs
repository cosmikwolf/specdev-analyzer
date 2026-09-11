//! Test items + test-aware validation.
//!
//! Coverage:
//!   * A `[[items]]` entry with `test = true` + `verifies` against a
//!     `nature = "test"` requirement validates clean.
//!   * `test_requirements_have_verifiers` warns (not errors) on a
//!     `nature = "test"` requirement that no test item verifies.
//!   * The module-orphan check (`requirements_have_satisfiers`) is
//!     unaffected — `nature = "test"` requirements remain exempt there.
//!   * `verifies` naming an unknown requirement surfaces REDUCE_UNRESOLVED.
//!   * Schema mutual-exclusion: `test = true` + `satisfies` rejected;
//!     non-test item carrying `verifies` rejected; `test = true` on a
//!     non-fn kind rejected.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml,
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
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
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
fn unverified_test_requirement_warns_not_errors() {
    // Drop the test item; REQ-TEST-001 now has no verifier.
    let module = r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
name = "value"
kind = "struct"
description = "The public value type."
satisfies = ["REQ-API-001"]
"#;
    let (target, _) = fresh_library("orphan-test");
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "orphan test req is a warning; got:\n{:#?}",
        errors(&report)
    );
    let warns = findings_for(
        &report,
        "test_requirements_have_verifiers",
        Severity::Warning,
    );
    assert_eq!(
        warns.len(),
        1,
        "exactly one unverified test req; got:\n{warns:#?}"
    );
    let msg = &warns[0].message;
    assert!(
        msg.contains("REQ-TEST-001"),
        "names the unverified req: {msg}"
    );
    assert!(msg.contains("test_verifies"), "names the relation: {msg}");
    // The module-orphan check must NOT fire on the test-nature req.
    let mod_warns = findings_for(&report, "requirements_have_satisfiers", Severity::Warning);
    assert!(
        mod_warns
            .iter()
            .all(|f| !f.message.contains("REQ-TEST-001")),
        "test req must not be flagged by the module-orphan check: {mod_warns:#?}"
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
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
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
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
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
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        !errors(&report).is_empty(),
        "a non-test item carrying verifies must fail schema validation"
    );
}

#[test]
fn reserved_test_kind_warns_no_emission() {
    // bench/fuzz reserve the enum slot (schema accepts them) but
    // emit nothing in v1 — validate warns (TEST-KIND-001).
    let module = r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
name = "throughput"
kind = "fn"
description = "Benchmarks frame throughput."
test = true
test_kind = "bench"
verifies = ["REQ-TEST-001"]
"#;
    let (target, _) = fresh_library("reserved-kind");
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    // bench validates clean (enum slot reserved) — no error.
    assert!(
        errors(&report).is_empty(),
        "bench is schema-valid; got:\n{:#?}",
        errors(&report)
    );
    let warns = findings_for(&report, "TEST-KIND-001", Severity::Warning);
    assert_eq!(
        warns.len(),
        1,
        "one reserved-kind warning; got:\n{warns:#?}"
    );
    assert!(
        warns[0].message.contains("bench"),
        "names the reserved kind: {}",
        warns[0].message
    );
}

#[test]
fn prompt_includes_test_item_verification_context() {
    use specdev_analyzer::prompt;
    let (target, layout) = fresh_library("prompt");
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", MODULE_WITH_TEST_ITEM);
    let a = prompt::assemble_module_prompt(&layout, "Lib").unwrap();
    let b = prompt::assemble_module_prompt(&layout, "Lib").unwrap();
    assert_eq!(a, b, "prompt assembly must be deterministic");
    assert!(
        a.contains("Test items (verification context)"),
        "test-items section present:\n{a}"
    );
    assert!(a.contains("roundtrip_serde"), "names the test item");
    assert!(
        a.contains("Round-trip serde tests run on every PR"),
        "injects the verifying requirement's text:\n{a}"
    );
    assert!(
        a.contains("REQ-TEST-001"),
        "names the verified requirement id"
    );
}

#[test]
fn test_only_external_dep_routes_to_dev_dependencies() {
    // A module external_dep flagged `test_only = true` lands in
    // `[dev-dependencies]`, not `[dependencies]`.
    let (target, _) = fresh_library("dev-deps");
    write(
        &target,
        "specs/requirements.toml",
        "[[requirement]]\nid = \"REQ-API-001\"\ntext = \"x\"\nnature = \"module\"\n",
    );
    write(
        &target,
        "specs/modules/Lib.toml",
        r#"name = "Lib"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[external_deps]]
crate = "serde"
version = "1"

[[external_deps]]
crate = "proptest"
version = "1"
test_only = true
"#,
    );
    let layout = LayoutContext::discover(&target, None).unwrap();
    let rendered = cargo_toml::render(&layout).unwrap();
    let text = &rendered.text;
    assert!(
        text.contains("[dev-dependencies]"),
        "dev-dependencies section emitted:\n{text}"
    );
    let (deps_part, dev_part) = text
        .split_once("[dev-dependencies]")
        .expect("has a dev-dependencies section");
    assert!(
        dev_part.contains("proptest"),
        "proptest must be a dev-dependency:\n{text}"
    );
    assert!(
        !deps_part.contains("proptest"),
        "proptest must NOT be a regular dependency:\n{text}"
    );
    assert!(
        deps_part.contains("serde"),
        "serde must be a regular dependency:\n{text}"
    );
    // `test_only` is a routing flag, never a rendered Cargo key.
    assert!(
        !text.contains("test_only"),
        "test_only must not leak into Cargo.toml:\n{text}"
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
    write(&target, "specs/requirements.toml", REQS_MODULE_AND_TEST);
    write(&target, "specs/modules/Lib.toml", module);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        !errors(&report).is_empty(),
        "test = true on kind = struct must fail schema validation (only fns can be tests)"
    );
}
