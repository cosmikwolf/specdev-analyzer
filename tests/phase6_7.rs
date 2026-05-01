//! Phase 6.7 integration tests: embedded CrateReference + per-profile
//! project-wide picks.
//!
//! Each test bootstraps a fresh firmware-layered or library-api project
//! from the Phase 6 fixture (the live plugin tree, post-6.7), then
//! mutates `specs/architecture.toml` / `specs/spec.toml` /
//! `specs/modules/*.toml` to exercise the new schema fields. Validation
//! findings drive the assertions.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    report::{Finding, Report},
    validate, Severity,
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
        "specdev-phase6_7-{tag}-{}-{}",
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

fn fresh_project(profile: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
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
// 1 — Live plugin snapshot bootstraps firmware-layered with full picks
//     and the default architecture.toml validates clean.
// ---------------------------------------------------------------------------
#[test]
fn firmware_default_template_validates() {
    let (target, _layout) = fresh_project("firmware-layered", "fw-default");
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "firmware-layered default template should validate clean; got:\n{}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 2 — Author full Nucleo-H563ZI-shaped architecture.toml; clean.
// ---------------------------------------------------------------------------
#[test]
fn firmware_architecture_with_full_picks_validates() {
    let (target, _layout) = fresh_project("firmware-layered", "fw-full");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"
[platform]
target_triple = "thumbv8m.main-none-eabihf"
mcu_part = "stm32h563"

[platform.mcu_core]
crate = "cortex-m"
version = "^0.7"

[platform.mcu_runtime]
crate = "cortex-m-rt"
version = "^0.7"

[runtime.panic_handler]
crate = "panic-probe"
version = "^0.3"
features = ["print-defmt"]

[runtime.allocator]
crate = "embedded-alloc"
version = "^0.5"

[logger.provider]
crate = "defmt"
version = "^0.3"

[logger.backend]
crate = "defmt-rtt"
version = "^0.4"

[[external_deps]]
crate = "heapless"
version = "^0.8"
features = ["serde"]

[[external_deps]]
crate = "static-cell"
version = "^2"
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
// 3 — Missing required field (panic_handler) → SCHEMA-ARCH.
// ---------------------------------------------------------------------------
#[test]
fn firmware_architecture_missing_panic_handler_rejects() {
    let (target, _layout) = fresh_project("firmware-layered", "fw-missing-panic");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"
[platform]
target_triple = "thumbv8m.main-none-eabihf"

[platform.mcu_core]
crate = "cortex-m"
version = "^0.7"

[platform.mcu_runtime]
crate = "cortex-m-rt"
version = "^0.7"

[runtime]
# panic_handler missing — required by the schema

[logger.provider]
crate = "defmt"
version = "^0.3"

[logger.backend]
crate = "defmt-rtt"
version = "^0.4"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"SCHEMA-ARCH".to_string()),
        "expected SCHEMA-ARCH for missing panic_handler; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 4 — Invalid crate name (uppercase) → SCHEMA-ARCH.
// ---------------------------------------------------------------------------
#[test]
fn firmware_architecture_invalid_crate_name_rejects() {
    let (target, _layout) = fresh_project("firmware-layered", "fw-bad-name");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"
[platform]
target_triple = "thumbv8m.main-none-eabihf"

[platform.mcu_core]
crate = "Cortex-M"
version = "^0.7"

[platform.mcu_runtime]
crate = "cortex-m-rt"
version = "^0.7"

[runtime.panic_handler]
crate = "panic-probe"
version = "^0.3"

[logger.provider]
crate = "defmt"
version = "^0.3"

[logger.backend]
crate = "defmt-rtt"
version = "^0.4"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"SCHEMA-ARCH".to_string()),
        "expected SCHEMA-ARCH for `Cortex-M` (uppercase); got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 5 — Invalid log level (not in enum) → SCHEMA-ARCH.
// ---------------------------------------------------------------------------
#[test]
fn firmware_architecture_invalid_log_level_rejects() {
    let (target, _layout) = fresh_project("firmware-layered", "fw-bad-level");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"
[platform]
target_triple = "thumbv8m.main-none-eabihf"

[platform.mcu_core]
crate = "cortex-m"
version = "^0.7"

[platform.mcu_runtime]
crate = "cortex-m-rt"
version = "^0.7"

[runtime.panic_handler]
crate = "panic-probe"
version = "^0.3"

[logger]
default_level = "verbose"

[logger.provider]
crate = "defmt"
version = "^0.3"

[logger.backend]
crate = "defmt-rtt"
version = "^0.4"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"SCHEMA-ARCH".to_string()),
        "expected SCHEMA-ARCH for `verbose` log level; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 6 — CrateReference features array: duplicates rejected.
// ---------------------------------------------------------------------------
#[test]
fn crate_reference_duplicate_features_reject() {
    let (target, _layout) = fresh_project("firmware-layered", "fw-dup-features");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"
[platform]
target_triple = "thumbv8m.main-none-eabihf"

[platform.mcu_core]
crate = "cortex-m"
version = "^0.7"

[platform.mcu_runtime]
crate = "cortex-m-rt"
version = "^0.7"

[runtime.panic_handler]
crate = "panic-probe"
version = "^0.3"
features = ["print-defmt", "print-defmt"]

[logger.provider]
crate = "defmt"
version = "^0.3"

[logger.backend]
crate = "defmt-rtt"
version = "^0.4"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"SCHEMA-ARCH".to_string()),
        "expected SCHEMA-ARCH for duplicate features; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 7 — library-api: module-level external_deps validates.
// ---------------------------------------------------------------------------
#[test]
fn library_module_with_external_deps_validates() {
    let (target, _layout) = fresh_project("library-api", "lib-extdeps");
    std::fs::write(
        target.join("specs/modules/Encoder.toml"),
        r#"name = "Encoder"
visibility = "pub"

[[external_deps]]
crate = "serde"
version = "^1"
features = ["derive"]
default_features = false

[[external_deps]]
crate = "thiserror"
version = "^1"
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
// 8 — library-api: architecture-level msrv validates.
// ---------------------------------------------------------------------------
#[test]
fn library_architecture_with_msrv_validates() {
    let (target, _layout) = fresh_project("library-api", "lib-msrv");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"
msrv = "1.75"

[[features]]
name = "std"
description = "Use Rust's std library."
default = true
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
// 9 — spec.toml [project] accepts edition/license/description.
// ---------------------------------------------------------------------------
#[test]
fn spec_with_project_extras_validates() {
    let (target, _layout) = fresh_project("library-api", "spec-extras");
    std::fs::write(
        target.join("spec.toml"),
        r#"
spec_version = "1.0"

[project]
name = "demo"
profile = "library-api"
language = "rust"
edition = "2024"
license = "MIT OR Apache-2.0"
description = "A short description for crates.io."

[paths]
specs = "specs"
source = "src"
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
// 10 — spec.toml rejects invalid edition (not in enum).
// ---------------------------------------------------------------------------
#[test]
fn spec_with_invalid_edition_rejects() {
    let (target, _layout) = fresh_project("library-api", "spec-bad-edition");
    std::fs::write(
        target.join("spec.toml"),
        r#"
spec_version = "1.0"

[project]
name = "demo"
profile = "library-api"
language = "rust"
edition = "2025"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    // spec.toml schema-validation runs at LayoutContext::discover. The
    // discover call itself fails when spec.toml violates its schema.
    let result = LayoutContext::discover(&target, None);
    assert!(
        result.is_err(),
        "expected LayoutContext::discover to fail for `edition = \"2025\"`"
    );
    let msg = format!("{:#}", result.err().unwrap());
    assert!(
        msg.contains("2025") || msg.contains("edition") || msg.contains("enum"),
        "error message should mention the bad edition; got: {msg}"
    );
}
