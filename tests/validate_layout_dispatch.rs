//! `validate.rs::validate_layout` surface-driven dispatch.
//!
//! `validate_layout` replaces the `layout.spec.workspace.is_some()`
//! no-module-surface heuristic with manifest-driven
//! dispatch off `profile.yaml.surface.has_modules` /
//! `surface.has_requirements`. The two suppressions are independently
//! controlled — today every shipped profile has `has_requirements` mirror
//! `has_modules`, but the schema admits asymmetric configurations.
//!
//! Coverage:
//!   1. rust-workspace (manifest path, both schemas suppressed) — bootstrap
//!      + validate emits NO `LAYOUT-003` for module.schema.yaml or
//!        requirements.schema.yaml.
//!   2. library-api (manifest path, both schemas required) — delete
//!      module.schema.yaml after bootstrap; validate WARNs LAYOUT-003.
//!   3. firmware-layered analogue.
//!   4. audio-plugin analogue.
//!   5. Manifest-less fallback: minimal_plugin fixture (no profile.yaml) +
//!      workspace-shaped spec.toml → fallback path suppresses BOTH schemas
//!      (mirrors the legacy heuristic; backward-compat preservation).
//!   6. Manifest-less fallback: minimal_plugin fixture + non-workspace
//!      spec.toml → both schemas trigger LAYOUT-003 if missing.
//!   7. Granularity: synthetic manifest with `has_modules: false` +
//!      `has_requirements: true` — only module.schema.yaml is suppressed;
//!      requirements.schema.yaml still WARNs when missing.
//!   8. Malformed profile.yaml during validate → MANIFEST-001 surfaces;
//!      no double-fire of LAYOUT-003 stacked on top.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    domain::Severity,
    layout::LayoutContext,
    report::Report,
    validate,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn minimal_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("minimal_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-validate-layout-dispatch-{tag}-{}-{}",
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

fn bootstrap_at(plugin: &Path, profile: &str, tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        plugin,
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

fn run_validate(target: &Path) -> Report {
    let layout = LayoutContext::discover(target, None).unwrap();
    let mut report = Report::default();
    validate::validate_layout(&layout, &mut report).unwrap();
    report
}

fn warns_for(report: &Report, schema_name: &str) -> bool {
    report.findings().iter().any(|f| {
        f.rule == "LAYOUT-003"
            && matches!(f.severity, Severity::Warning)
            && f.message.contains(schema_name)
    })
}

// ===========================================================================
// 1 — rust-workspace manifest path: both surface schemas suppressed
// ===========================================================================

#[test]
fn rust_workspace_manifest_suppresses_both_schemas() {
    let target = bootstrap_at(&full_plugin(), "rust-workspace", "ws-suppress");
    // Sanity: the manifest is on disk and declares the false surface.
    let manifest = std::fs::read_to_string(target.join("specs/specdev/profile.yaml")).unwrap();
    assert!(manifest.contains("has_modules: false"));
    assert!(manifest.contains("has_requirements: false"));
    let report = run_validate(&target);
    assert!(
        !warns_for(&report, "module.schema.yaml"),
        "manifest declares has_modules: false; LAYOUT-003 must NOT warn for module.schema.yaml.\n{}",
        report.render_plain()
    );
    assert!(
        !warns_for(&report, "requirements.schema.yaml"),
        "manifest declares has_requirements: false; LAYOUT-003 must NOT warn for requirements.schema.yaml.\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 2 — library-api manifest path: schemas required; deletion makes LAYOUT-003 fire
// ===========================================================================

#[test]
fn library_api_manifest_warns_when_module_schema_missing() {
    let target = bootstrap_at(&full_plugin(), "library-api", "lib-warn");
    // Premise: manifest has has_modules: true.
    let manifest = std::fs::read_to_string(target.join("specs/specdev/profile.yaml")).unwrap();
    assert!(manifest.contains("has_modules: true"));
    // Force the missing-warning path.
    std::fs::remove_file(target.join("specs/specdev/schemas/module.schema.yaml")).unwrap();
    let report = run_validate(&target);
    assert!(
        warns_for(&report, "module.schema.yaml"),
        "library-api MUST warn LAYOUT-003 when module.schema.yaml is missing.\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 3 — firmware-layered manifest path
// ===========================================================================

#[test]
fn firmware_layered_manifest_warns_when_requirements_schema_missing() {
    let target = bootstrap_at(&full_plugin(), "firmware-layered", "fw-warn");
    let manifest = std::fs::read_to_string(target.join("specs/specdev/profile.yaml")).unwrap();
    assert!(manifest.contains("has_requirements: true"));
    std::fs::remove_file(target.join("specs/specdev/schemas/requirements.schema.yaml")).unwrap();
    let report = run_validate(&target);
    assert!(
        warns_for(&report, "requirements.schema.yaml"),
        "firmware-layered MUST warn LAYOUT-003 when requirements.schema.yaml is missing.\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 4 — audio-plugin manifest path
// ===========================================================================

#[test]
fn audio_plugin_manifest_warns_when_module_schema_missing() {
    let target = bootstrap_at(&full_plugin(), "audio-plugin", "ap-warn");
    let manifest = std::fs::read_to_string(target.join("specs/specdev/profile.yaml")).unwrap();
    assert!(manifest.contains("has_modules: true"));
    std::fs::remove_file(target.join("specs/specdev/schemas/module.schema.yaml")).unwrap();
    let report = run_validate(&target);
    assert!(
        warns_for(&report, "module.schema.yaml"),
        "audio-plugin MUST warn LAYOUT-003 when module.schema.yaml is missing.\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 5 — Manifest-less fallback: no profile.yaml + workspace spec.toml → suppress both
// ===========================================================================

#[test]
fn manifest_less_fallback_workspace_suppresses_both_schemas() {
    // The minimal_plugin fixture inlines no profile.yaml. We need a
    // workspace-shaped project to exercise the legacy heuristic path:
    // write a synthetic spec.toml carrying [workspace] and run validate
    // against it directly (the minimal_plugin fixture has no rust-workspace
    // profile, so we synthesize the project tree manually).
    let workspace = tmp_workspace("manifest-less-ws");
    let target = workspace.join("proj");
    std::fs::create_dir_all(target.join("specs/specdev/schemas")).unwrap();
    std::fs::create_dir_all(target.join("specs/specdev/codegen/rust")).unwrap();
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "ws-manifest-less"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."

[workspace]

[[workspace.members]]
name = "placeholder"
path = "crates/placeholder"
"#,
    )
    .unwrap();
    // Provide aggregator + reduction-required files so validate_layout
    // doesn't error before reaching the suppression site.
    std::fs::write(
        target.join("specs/specdev/schemas/aggregator.schema.yaml"),
        "$schema: https://json-schema.org/draft/2020-12/schema\ntype: object\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/specdev/schemas/architecture.schema.yaml"),
        "$schema: https://json-schema.org/draft/2020-12/schema\ntype: object\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/specdev/reduction.yaml"),
        "kinds: []\nrelations: []\n",
    )
    .unwrap();
    // Confirm test premise: NO profile.yaml.
    assert!(!target.join("specs/specdev/profile.yaml").exists());
    let report = run_validate(&target);
    assert!(
        !warns_for(&report, "module.schema.yaml"),
        "manifest-less workspace fallback must suppress module.schema.yaml.\n{}",
        report.render_plain()
    );
    assert!(
        !warns_for(&report, "requirements.schema.yaml"),
        "manifest-less workspace fallback must suppress requirements.schema.yaml.\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 6 — Manifest-less fallback: no profile.yaml + non-workspace spec.toml → both warn
// ===========================================================================

#[test]
fn manifest_less_fallback_non_workspace_warns_both_schemas() {
    let target = bootstrap_at(&minimal_plugin(), "firmware-layered", "manifest-less-nonws");
    // Premise: the minimal_plugin bootstrap does NOT inline profile.yaml.
    assert!(
        !target.join("specs/specdev/profile.yaml").exists(),
        "minimal_plugin fixture must NOT inline profile.yaml (test premise)"
    );
    // Force missing for both schemas.
    let _ = std::fs::remove_file(target.join("specs/specdev/schemas/module.schema.yaml"));
    let _ = std::fs::remove_file(target.join("specs/specdev/schemas/requirements.schema.yaml"));
    let report = run_validate(&target);
    assert!(
        warns_for(&report, "module.schema.yaml"),
        "manifest-less non-workspace fallback MUST warn LAYOUT-003 for module.schema.yaml.\n{}",
        report.render_plain()
    );
    assert!(
        warns_for(&report, "requirements.schema.yaml"),
        "manifest-less non-workspace fallback MUST warn LAYOUT-003 for requirements.schema.yaml.\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 7 — Granularity: asymmetric surface (modules false / requirements true)
// ===========================================================================

#[test]
fn asymmetric_surface_suppresses_only_module_not_requirements() {
    // Bootstrap library-api, then mutate its inlined profile.yaml so
    // surface declares has_modules: false + has_requirements: true.
    // Delete BOTH schemas; only requirements.schema.yaml should warn.
    let target = bootstrap_at(&full_plugin(), "library-api", "asym");
    let manifest_path = target.join("specs/specdev/profile.yaml");
    let body = std::fs::read_to_string(&manifest_path).unwrap();
    let mutated = body.replace(
        "surface:\n  has_modules: true\n  has_requirements: true",
        "surface:\n  has_modules: false\n  has_requirements: true",
    );
    assert_ne!(
        body, mutated,
        "fixture surface block did not match expected source text"
    );
    std::fs::write(&manifest_path, &mutated).unwrap();

    let _ = std::fs::remove_file(target.join("specs/specdev/schemas/module.schema.yaml"));
    let _ = std::fs::remove_file(target.join("specs/specdev/schemas/requirements.schema.yaml"));
    let report = run_validate(&target);
    assert!(
        !warns_for(&report, "module.schema.yaml"),
        "asymmetric surface (modules false) must suppress module.schema.yaml.\n{}",
        report.render_plain()
    );
    assert!(
        warns_for(&report, "requirements.schema.yaml"),
        "asymmetric surface (requirements true) must NOT suppress requirements.schema.yaml.\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// 8 — Malformed profile.yaml: MANIFEST-001 surfaces; no double-fire LAYOUT-003
// ===========================================================================

#[test]
fn malformed_profile_yaml_emits_manifest_001_no_double_fire() {
    let target = bootstrap_at(&full_plugin(), "library-api", "malformed");
    // Overwrite the manifest with a body that fails schema validation
    // (missing required `name` + `language`).
    std::fs::write(target.join("specs/specdev/profile.yaml"), "garbage: true\n").unwrap();
    // Keep both schemas present so the missing-warning path doesn't fire
    // for those — this isolates the assertion on MANIFEST-001 surface.
    let report = run_validate(&target);
    let manifest_findings: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| f.rule == "MANIFEST-001")
        .collect();
    assert!(
        !manifest_findings.is_empty(),
        "malformed profile.yaml must surface MANIFEST-001:\n{}",
        report.render_plain()
    );
    // Defensive: malformed manifest path falls back to the legacy
    // workspace.is_some() heuristic. spec.toml here has no [workspace],
    // so suppression resolves to false. With both schemas present, no
    // LAYOUT-003 warning fires for either schema.
    assert!(
        !warns_for(&report, "module.schema.yaml"),
        "schemas are present; LAYOUT-003 should not warn even on manifest failure.\n{}",
        report.render_plain()
    );
    assert!(
        !warns_for(&report, "requirements.schema.yaml"),
        "schemas are present; LAYOUT-003 should not warn even on manifest failure.\n{}",
        report.render_plain()
    );
}
