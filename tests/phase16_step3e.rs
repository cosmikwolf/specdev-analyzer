//! Phase 16 §8.1 step 3e — `PROFILE-LAYOUT-MISMATCH` validate-time check.
//!
//! Per doc 21 §3.5: `cargo_toml.shape: workspace` in `profile.yaml` is
//! descriptive metadata. Actual workspace dispatch is layout-driven (the
//! `[workspace]` table in spec.toml is canonical per D36). When the two
//! disagree, the project is in an unrenderable state — surfacing the
//! mismatch at validate time gives the user a structural prompt before
//! they hit the cargo-toml render path.
//!
//! Step 3e is **additive**: no existing finding semantics change; one
//! new diagnostic code (`PROFILE-LAYOUT-MISMATCH`) lights up only when
//! the inlined `profile.yaml` and the spec.toml `[workspace]` table
//! disagree.
//!
//! Step 3e coverage:
//!   1. `shape: workspace` + `[workspace]` table → no finding (consistent).
//!   2. `shape: workspace` + no `[workspace]` table → ERROR finding.
//!   3. non-workspace shape + `[workspace]` table → ERROR finding.
//!   4. non-workspace shape + no `[workspace]` table → no finding.
//!   5. pre-D52 fixture (no `profile.yaml`) → no finding (backward-compat).
//!   6. malformed `profile.yaml` → MANIFEST-001 finding (load failure
//!      surfaces here too so consumers see it without running render).

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    domain::Severity,
    layout::LayoutContext,
    report::Report,
    validate::validate_profile_layout_match,
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn phase3_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase3")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase16-step3e-{tag}-{}-{}",
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

fn fresh_project(plugin: &Path, profile: &str, name: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        plugin,
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: name.into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn run_check(layout: &LayoutContext) -> Report {
    let mut report = Report::default();
    validate_profile_layout_match(layout, &mut report);
    report
}

fn has_finding(report: &Report, code: &str) -> bool {
    report.findings().iter().any(|f| f.rule == code)
}

fn mismatch_finding(report: &Report) -> Option<&specdev_analyzer::report::Finding> {
    report
        .findings()
        .iter()
        .find(|f| f.rule == "PROFILE-LAYOUT-MISMATCH")
}

// ===========================================================================
// 1 — Workspace shape + [workspace] table → consistent, no finding
// ===========================================================================

#[test]
fn workspace_shape_with_workspace_table_passes() {
    // Default rust-workspace bootstrap declares [workspace] in spec.toml,
    // and ships shape: workspace in profile.yaml. The two agree.
    let (_target, layout) = fresh_project(
        &phase6_plugin(),
        "rust-workspace",
        "demo-ws",
        "ws-with-table",
    );
    assert!(layout.spec.workspace.is_some(), "[workspace] table expected");
    let report = run_check(&layout);
    assert!(
        !has_finding(&report, "PROFILE-LAYOUT-MISMATCH"),
        "consistent profile + layout must not raise the mismatch finding:\n{}",
        report.render_plain(),
    );
}

// ===========================================================================
// 2 — Workspace shape + no [workspace] table → ERROR finding
// ===========================================================================

#[test]
fn workspace_shape_without_workspace_table_emits_error() {
    let (target, _initial_layout) = fresh_project(
        &phase6_plugin(),
        "rust-workspace",
        "broken",
        "ws-no-table",
    );
    // Strip [workspace] from spec.toml — manifest still claims
    // shape: workspace, so the validate check must flag the mismatch.
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "broken"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."
"#,
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let report = run_check(&layout);
    let f = mismatch_finding(&report).unwrap_or_else(|| {
        panic!(
            "expected PROFILE-LAYOUT-MISMATCH error, got:\n{}",
            report.render_plain()
        )
    });
    assert_eq!(f.severity, Severity::Error, "must be Error severity");
    assert!(
        f.message.contains("workspace"),
        "diagnostic must reference workspace:\n{}",
        f.message
    );
    assert!(
        f.message.contains("[workspace]"),
        "diagnostic must reference the missing [workspace] table:\n{}",
        f.message
    );
    // D51 vocabulary: do not name the specific profile.
    assert!(
        !f.message.contains("rust-workspace"),
        "diagnostic must NOT name the specific profile (D51):\n{}",
        f.message
    );
}

// ===========================================================================
// 3 — Non-workspace shape + [workspace] table → ERROR finding
// ===========================================================================

#[test]
fn non_workspace_shape_with_workspace_table_emits_error() {
    // Bootstrap library-api (shape: library) then inject a [workspace]
    // table into spec.toml. The manifest still claims shape: library,
    // so the validate check must flag the mismatch.
    let (target, _initial_layout) =
        fresh_project(&phase6_plugin(), "library-api", "liblib", "lib-with-ws");
    let spec_path = target.join("spec.toml");
    let original = std::fs::read_to_string(&spec_path).unwrap();
    let injected = format!(
        "{original}\n[[workspace.members]]\nname = \"placeholder\"\npath = \"crates/placeholder\"\n"
    );
    std::fs::write(&spec_path, injected).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    assert!(
        layout.spec.workspace.is_some(),
        "test premise: [workspace] table must parse"
    );
    let report = run_check(&layout);
    let f = mismatch_finding(&report).unwrap_or_else(|| {
        panic!(
            "expected PROFILE-LAYOUT-MISMATCH error, got:\n{}",
            report.render_plain()
        )
    });
    assert_eq!(f.severity, Severity::Error);
    assert!(
        f.message.contains("[workspace]"),
        "diagnostic must reference [workspace]:\n{}",
        f.message
    );
    assert!(
        f.message.contains("non-workspace") || f.message.contains("workspace"),
        "diagnostic must reference shape:\n{}",
        f.message
    );
    // D51: no profile name in the message.
    assert!(
        !f.message.contains("library-api"),
        "diagnostic must NOT name the specific profile (D51):\n{}",
        f.message
    );
}

// ===========================================================================
// 4 — Non-workspace shape + no [workspace] table → consistent, no finding
// ===========================================================================

#[test]
fn non_workspace_shape_without_workspace_table_passes() {
    // library-api / firmware-layered / audio-plugin all bootstrap with
    // no [workspace] table and a non-workspace shape — the consistent
    // baseline that should never raise the finding.
    for (profile, tag) in [
        ("library-api", "lib-clean"),
        ("firmware-layered", "fw-clean"),
        ("audio-plugin", "ap-clean"),
    ] {
        let (_target, layout) = fresh_project(&phase6_plugin(), profile, "x", tag);
        assert!(layout.spec.workspace.is_none());
        let report = run_check(&layout);
        assert!(
            !has_finding(&report, "PROFILE-LAYOUT-MISMATCH"),
            "consistent {profile} project must not raise the mismatch finding:\n{}",
            report.render_plain(),
        );
    }
}

// ===========================================================================
// 5 — Pre-D52 fixture (no profile.yaml) → silent no-op
// ===========================================================================

#[test]
fn pre_d52_fixture_without_manifest_silently_skips() {
    // phase3 fixture predates D52 — its bootstrap does NOT write
    // profile.yaml. The check must silently no-op (no finding).
    assert!(
        !phase3_plugin()
            .join("profiles/firmware-layered/profile.yaml")
            .exists(),
        "phase3 fixture must NOT ship profile.yaml (test premise)"
    );
    let (target, layout) = fresh_project(&phase3_plugin(), "firmware-layered", "demo", "phase3");
    assert!(
        !target.join("specs/specdev/profile.yaml").exists(),
        "phase3 bootstrap must NOT write profile.yaml"
    );
    let report = run_check(&layout);
    assert!(
        report.findings().is_empty(),
        "absent profile.yaml must produce zero findings:\n{}",
        report.render_plain(),
    );
}

// ===========================================================================
// 6 — Malformed profile.yaml surfaces as MANIFEST-001 from the validate path
// ===========================================================================

#[test]
fn malformed_profile_yaml_emits_manifest_001() {
    let (target, _initial_layout) =
        fresh_project(&phase6_plugin(), "library-api", "liblib", "bad-yaml");
    // Overwrite the inlined profile.yaml with content that fails schema
    // validation (missing required `name` + `language` fields).
    let manifest = target.join("specs/specdev/profile.yaml");
    std::fs::write(&manifest, "garbage: true\n").unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let report = run_check(&layout);
    assert!(
        has_finding(&report, "MANIFEST-001"),
        "malformed profile.yaml must surface as MANIFEST-001:\n{}",
        report.render_plain(),
    );
    // No mismatch finding when the manifest didn't load — we only
    // know the layout half of the comparison.
    assert!(
        !has_finding(&report, "PROFILE-LAYOUT-MISMATCH"),
        "unloadable manifest must NOT also fire PROFILE-LAYOUT-MISMATCH:\n{}",
        report.render_plain(),
    );
}
