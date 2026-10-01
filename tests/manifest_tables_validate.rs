//! Deterministic Cargo.toml emission.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
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
        "specdev-cargo-toml-emission-{tag}-{}-{}",
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

fn fresh_project(profile: &str, name: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
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

// A test target is a closed subset: `required-features`, `bench`,
// `test = false` and every other Cargo key are rejected until a project
// brings a witness for them.
#[test]
fn firmware_test_target_rejects_keys_outside_the_subset() {
    let (target, _layout) = fresh_project("firmware-layered", "demo", "fw-test-target-closed");
    let arch = std::fs::read_to_string(target.join("specs/architecture.toml")).unwrap();
    std::fs::write(
        target.join("specs/architecture.toml"),
        format!("{arch}\n[[test_targets]]\nname = \"hw\"\nbench = false\n"),
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        report
            .findings()
            .iter()
            .any(|f| matches!(f.severity, Severity::Error) && f.message.contains("bench")),
        "an unknown key on a test target must be a validate error:\n{}",
        report.render_plain()
    );
}

// `depends_on` resolves through the feature_depends_on relation:
// an unknown feature name is REDUCE_UNRESOLVED.
#[test]
fn firmware_feature_depends_on_unknown_feature_fails_validate() {
    let (target, _layout) = fresh_project("firmware-layered", "demo", "fw-features-ghost");
    let arch = std::fs::read_to_string(target.join("specs/architecture.toml")).unwrap();
    std::fs::write(
        target.join("specs/architecture.toml"),
        format!("{arch}\n[[features]]\nname = \"alpha\"\ndepends_on = [\"ghost\"]\n"),
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.rule == "REDUCE_UNRESOLVED"
                && f.message.contains("feature_depends_on")
                && f.message.contains("`ghost`")),
        "expected REDUCE_UNRESOLVED naming feature_depends_on and `ghost`:\n{}",
        report.render_plain()
    );
}

// Duplicate feature names fail the feature_unique_names rule.
#[test]
fn firmware_duplicate_feature_names_fail_validate() {
    let (target, _layout) = fresh_project("firmware-layered", "demo", "fw-features-dup");
    let arch = std::fs::read_to_string(target.join("specs/architecture.toml")).unwrap();
    std::fs::write(
        target.join("specs/architecture.toml"),
        format!("{arch}\n[[features]]\nname = \"alpha\"\n\n[[features]]\nname = \"alpha\"\n"),
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        report
            .findings()
            .iter()
            .any(|f| f.rule == "feature_unique_names" && matches!(f.severity, Severity::Error)),
        "expected a feature_unique_names error:\n{}",
        report.render_plain()
    );
}
