//! First profiles end-to-end.
//!
//! Behaviors under test:
//!
//! 1. Bootstrap firmware-layered → validate produces 0 errors.
//! 2. Bootstrap library-api → validate produces 0 errors.
//! 3. Firmware: one minimal module validates.
//! 4. Library: one minimal module validates.
//! 5. Firmware: HAL → application dependency triggers `layer_dependency`.
//! 6. Library: legacy `[[re_exports]]` table is rejected by the schema.
//! 7. Firmware: full e2e with architecture + requirements cross-schema.
//! 8. Library: full e2e with feature graph + requirements cross-schema.
//! 9. Firmware: unknown port endpoint triggers `connection_endpoints_resolve`.
//! 10. Library: unknown feature gate triggers `module_feature_gates_resolve`.
//! 11. Architecture-schema violation triggers `SCHEMA-ARCH`.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    report::{Finding, Report},
    validate, Severity,
};

fn dual_profile_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("dual_profile_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-profiles-e2e-{tag}-{}-{}",
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

fn fresh_bootstrap(profile: &str, tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &dual_profile_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

fn errors(report: &Report) -> Vec<&Finding> {
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect()
}

fn rule_codes(report: &Report) -> Vec<String> {
    let mut out: Vec<String> = report
        .findings()
        .iter()
        .map(|f| f.rule.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

// ---------------------------------------------------------------------------
// 1 — Bootstrap firmware-layered + validate clean.
// ---------------------------------------------------------------------------
#[test]
fn bootstrap_firmware_validates_clean() {
    let target = fresh_bootstrap("firmware-layered", "fw-bootstrap");
    let report = validate::run_project(&target, None).unwrap();
    let errs = errors(&report);
    assert!(errs.is_empty(), "expected 0 errors:\n{}", report.render_plain());
    // Sanity: profile-shipped files landed.
    assert!(target.join("specs/architecture.toml").is_file());
    assert!(target.join("specs/requirements.toml").is_file());
    assert!(target
        .join("specs/specdev/schemas/architecture.schema.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/schemas/requirements.schema.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/schemas/enums.schema.yaml")
        .is_file());
    // firmware-layered ships metadata.schema.yaml; library-api doesn't.
    assert!(target
        .join("specs/specdev/schemas/metadata.schema.yaml")
        .is_file());
}

// ---------------------------------------------------------------------------
// 2 — Bootstrap library-api + validate clean.
// ---------------------------------------------------------------------------
#[test]
fn bootstrap_library_validates_clean() {
    let target = fresh_bootstrap("library-api", "lib-bootstrap");
    let report = validate::run_project(&target, None).unwrap();
    let errs = errors(&report);
    assert!(errs.is_empty(), "expected 0 errors:\n{}", report.render_plain());
    // library-api ships its own enums.schema.yaml but no metadata.
    assert!(target
        .join("specs/specdev/schemas/enums.schema.yaml")
        .is_file());
    assert!(!target
        .join("specs/specdev/schemas/metadata.schema.yaml")
        .is_file());
}

// ---------------------------------------------------------------------------
// 3 — Firmware: one minimal module validates.
// ---------------------------------------------------------------------------
#[test]
fn firmware_minimal_module_validates() {
    let target = fresh_bootstrap("firmware-layered", "fw-min-mod");
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
layer = "driver"
description = "Minimal stub"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 4 — Library: one minimal module validates.
// ---------------------------------------------------------------------------
#[test]
fn library_minimal_module_validates() {
    let target = fresh_bootstrap("library-api", "lib-min-mod");
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"
description = "Minimal stub"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 5 — Firmware: HAL depending on application is an upward-edge violation.
// ---------------------------------------------------------------------------
#[test]
fn firmware_layer_dependency_violation() {
    let target = fresh_bootstrap("firmware-layered", "fw-layer-vio");
    std::fs::write(
        target.join("specs/modules/AppMod.toml"),
        r#"name = "AppMod"
layer = "application"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/HalMod.toml"),
        r#"name = "HalMod"
layer = "HAL"
depends_on = ["AppMod"]
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"layer_dependency".to_string()),
        "expected layer_dependency finding; got rules: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 6 — Library: legacy `[[re_exports]]` table is rejected by the schema.
//     Re-exports now live under `[[items]]`
//     with `kind = "re_export"`. The schema's `unevaluatedProperties:
//     false` makes the unknown top-level key fatal.
// ---------------------------------------------------------------------------
#[test]
fn library_legacy_re_exports_field_rejected() {
    let target = fresh_bootstrap("library-api", "lib-reexp");
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[re_exports]]
target = "Nonexistent"
visibility = "pub"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let codes = rule_codes(&report);
    assert!(
        codes.iter().any(|c| c.starts_with("SCHEMA")),
        "expected SCHEMA finding for unknown top-level `re_exports`; got: {:?}\n{}",
        codes,
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 7 — Firmware: end-to-end cross-schema.
// ---------------------------------------------------------------------------
#[test]
fn firmware_e2e_cross_schema() {
    let target = fresh_bootstrap("firmware-layered", "fw-e2e");
    // Two modules with ports.
    std::fs::write(
        target.join("specs/modules/Source.toml"),
        r#"name = "Source"
layer = "driver"
satisfies = ["REQ-AUDIO-001"]

[[ports]]
name = "out"
port_type = "audio"
direction = "out"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Sink.toml"),
        r#"name = "Sink"
layer = "driver"

[[ports]]
name = "in"
port_type = "audio"
direction = "in"
"#,
    )
    .unwrap();
    // Architecture: connect Source.out → Sink.in.
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[[connections]]
from = "Source.out"
to = "Sink.in"
flow_type = "audio_frame"
"#,
    )
    .unwrap();
    // Requirements: REQ-AUDIO-001.
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-AUDIO-001"
text = "Audio output shall not drop frames."
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 8 — Library: end-to-end cross-schema with feature graph.
// ---------------------------------------------------------------------------
#[test]
fn library_e2e_cross_schema() {
    let target = fresh_bootstrap("library-api", "lib-e2e");
    std::fs::write(
        target.join("specs/modules/Core.toml"),
        r#"name = "Core"
visibility = "pub"
satisfies = ["REQ-API-001"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Optional.toml"),
        r#"name = "Optional"
visibility = "pub"
feature_gates = ["alloc"]
depends_on = ["Core"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[[features]]
name = "std"
default = true

[[features]]
name = "alloc"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-API-001"
text = "Public types shall implement Debug."
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 9 — Firmware: connection endpoint references nonexistent port.
// ---------------------------------------------------------------------------
#[test]
fn firmware_unknown_port_endpoint() {
    let target = fresh_bootstrap("firmware-layered", "fw-unknown-port");
    std::fs::write(
        target.join("specs/modules/Mod.toml"),
        r#"name = "Mod"
layer = "driver"

[[ports]]
name = "out"
port_type = "x"
direction = "out"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[[connections]]
from = "Mod.out"
to = "Nonexistent.in"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"connection_endpoints_resolve".to_string()),
        "expected connection_endpoints_resolve; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 10 — Library: feature_gates references undeclared feature.
// ---------------------------------------------------------------------------
#[test]
fn library_unknown_feature_gate() {
    let target = fresh_bootstrap("library-api", "lib-unknown-feat");
    std::fs::write(
        target.join("specs/modules/Mod.toml"),
        r#"name = "Mod"
visibility = "pub"
feature_gates = ["nonexistent"]
"#,
    )
    .unwrap();
    // Architecture is empty (no features declared).
    let report = validate::run_project(&target, None).unwrap();
    // Same as re_exports: REDUCE_UNRESOLVED fires at executor time
    // because module_feature_gate's `to_kind: Feature` doesn't resolve.
    let codes = rule_codes(&report);
    assert!(
        codes.contains(&"REDUCE_UNRESOLVED".to_string()),
        "expected REDUCE_UNRESOLVED for missing feature_gate target; got: {:?}\n{}",
        codes,
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 11 — Architecture-schema violation triggers SCHEMA-ARCH.
// ---------------------------------------------------------------------------
#[test]
fn architecture_schema_violation() {
    let target = fresh_bootstrap("library-api", "arch-bad");
    // Feature with invalid name shape (uppercase letters not allowed).
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[[features]]
name = "BadName"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"SCHEMA-ARCH".to_string()),
        "expected SCHEMA-ARCH; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}
