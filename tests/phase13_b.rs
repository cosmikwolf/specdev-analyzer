//! Phase 13-B integration tests — `[project].handoff_mode` field on
//! `spec.toml` (doc 16 §4). Schema accepts `"implementation"` /
//! `"skeleton"`; absent ⇒ `None`. Other values rejected.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    config::{HandoffMode, SpecConfig},
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase13b-{tag}-{}-{}",
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

fn analyzer_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("specdev-analyzer")
}

fn run_validate(project: &Path) -> (bool, String, String) {
    let out = Command::new(analyzer_bin())
        .arg("validate")
        .arg("--project")
        .arg(project)
        .arg("--no-color")
        .output()
        .expect("invoke validate");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn fresh_library_project(tag: &str) -> PathBuf {
    let target = tmp_dir(tag);
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

#[test]
fn handoff_mode_absent_parses_as_none() {
    let target = fresh_library_project("absent");
    let cfg = SpecConfig::load(&target.join("spec.toml")).unwrap();
    assert!(cfg.project.handoff_mode.is_none());
}

#[test]
fn handoff_mode_implementation_parses() {
    let target = fresh_library_project("impl");
    let spec_path = target.join("spec.toml");
    let mut text = std::fs::read_to_string(&spec_path).unwrap();
    text = text.replace(
        "[project]",
        "[project]\nhandoff_mode = \"implementation\"",
    );
    std::fs::write(&spec_path, text).unwrap();
    let cfg = SpecConfig::load(&spec_path).unwrap();
    assert_eq!(
        cfg.project.handoff_mode,
        Some(HandoffMode::Implementation)
    );
    let (success, stdout, stderr) = run_validate(&target);
    let combined = format!("{stdout}\n{stderr}");
    // validate may still report unrelated layout warnings, but no SCHEMA
    // findings against handoff_mode.
    assert!(
        !combined.contains("handoff_mode"),
        "handoff_mode = \"implementation\" should validate clean against the spec.toml schema; out:\n{combined}"
    );
    let _ = success;
}

#[test]
fn handoff_mode_skeleton_parses() {
    let target = fresh_library_project("skel");
    let spec_path = target.join("spec.toml");
    let mut text = std::fs::read_to_string(&spec_path).unwrap();
    text = text.replace(
        "[project]",
        "[project]\nhandoff_mode = \"skeleton\"",
    );
    std::fs::write(&spec_path, text).unwrap();
    let cfg = SpecConfig::load(&spec_path).unwrap();
    assert_eq!(cfg.project.handoff_mode, Some(HandoffMode::Skeleton));
}

#[test]
fn handoff_mode_unknown_value_rejected() {
    let target = fresh_library_project("bad");
    let spec_path = target.join("spec.toml");
    let mut text = std::fs::read_to_string(&spec_path).unwrap();
    text = text.replace(
        "[project]",
        "[project]\nhandoff_mode = \"draft\"",
    );
    std::fs::write(&spec_path, text).unwrap();
    let err = SpecConfig::load(&spec_path).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("handoff_mode") || msg.contains("draft"),
        "expected schema error to mention the bad field/value; got:\n{msg}"
    );
}
