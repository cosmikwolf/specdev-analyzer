//! `validate_layout` surface-driven suppression of the `specs/modules/`
//! directory entry (F-009).
//!
//! The per-module / per-requirement schema FILE warnings are gated on
//! `surface.has_modules` / `surface.has_requirements`. The same gate
//! extends to the `specs/modules/` DIRECTORY entry: workspace-shell
//! profiles like `rust-workspace` declare no module surface, so a missing
//! `specs/modules/` there is by-design rather than a layout violation.
//!
//! Coverage:
//!   1. rust-workspace manifest path: `specs/modules/` absent → no
//!      LAYOUT-003 (suppressed via has_modules: false).
//!   2. library-api manifest path: `specs/modules/` absent → no LAYOUT-003
//!      either. This test used to require the warning; it changed when an
//!      absent directory that holds only project-added files came to mean
//!      "none yet": git stores no empty directory, so every clone of a
//!      project with no module specs lacks it. `has_modules` still gates the
//!      module schema FILE warning.

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

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-validate-layout-suppr-{tag}-{}-{}",
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

fn warns_for_modules_dir(report: &Report) -> bool {
    report.findings().iter().any(|f| {
        f.rule == "LAYOUT-003"
            && matches!(f.severity, Severity::Warning)
            && f.message.contains("specs/modules/")
    })
}

#[test]
fn rust_workspace_manifest_suppresses_specs_modules_directory() {
    let target = bootstrap_at(&full_plugin(), "rust-workspace", "ws-modules-dir");
    // Sanity: manifest declares the false surface.
    let manifest = std::fs::read_to_string(target.join("specs/specdev/profile.yaml")).unwrap();
    assert!(manifest.contains("has_modules: false"));
    // Force missing: rust-workspace bootstrap creates an empty specs/modules/;
    // remove it to exercise the absent-warning path.
    let modules_dir = target.join("specs/modules");
    if modules_dir.is_dir() {
        std::fs::remove_dir_all(&modules_dir).unwrap();
    }
    assert!(
        !modules_dir.exists(),
        "specs/modules/ must be absent for this test"
    );
    let report = run_validate(&target);
    assert!(
        !warns_for_modules_dir(&report),
        "manifest declares has_modules: false; LAYOUT-003 must NOT warn for specs/modules/.\n{}",
        report.render_plain()
    );
}

#[test]
fn library_api_manifest_does_not_warn_when_specs_modules_directory_missing() {
    let target = bootstrap_at(&full_plugin(), "library-api", "lib-modules-dir");
    let manifest = std::fs::read_to_string(target.join("specs/specdev/profile.yaml")).unwrap();
    assert!(manifest.contains("has_modules: true"));
    let modules_dir = target.join("specs/modules");
    if modules_dir.is_dir() {
        std::fs::remove_dir_all(&modules_dir).unwrap();
    }
    assert!(
        !modules_dir.exists(),
        "specs/modules/ must be absent for this test"
    );
    let report = run_validate(&target);
    assert!(
        report.render_plain().contains("DOMAIN-001"),
        "validate did not run to the end.\n{}",
        report.render_plain()
    );
    assert!(
        !warns_for_modules_dir(&report),
        "an absent specs/modules/ means no module specs yet; LAYOUT-003 must NOT warn.\n{}",
        report.render_plain()
    );
}
