//! The firmware-layered architecture template validates as bootstrapped.
//!
//! (The project-wide dependency picks this file once covered — platform,
//! runtime and logger crates, `external_deps`, `msrv`, `[project]` package
//! fields — left the spec in the 2.0 format: the manifest holds them.)

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
        "specdev-crate-reference-picks-{tag}-{}-{}",
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
        &full_plugin(),
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

// ---------------------------------------------------------------------------
// 3 — Missing required field (panic_handler) → SCHEMA-ARCH.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 4 — Invalid crate name (uppercase) → SCHEMA-ARCH.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 5 — Invalid log level (not in enum) → SCHEMA-ARCH.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 6 — CrateReference features array: duplicates rejected.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 7 — library-api: module-level external_deps validates.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 8 — library-api: architecture-level msrv validates.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 9 — spec.toml [project] accepts edition/license/description.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 10 — spec.toml rejects invalid edition (not in enum).
// ---------------------------------------------------------------------------
