//! The gate registry (`gates:` in `profile.yaml`) is checked when the
//! profile loads: duplicate labels and malformed entries are refused before
//! any gate can run.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::layout::LayoutContext;
use specdev_analyzer::manifest::Profile;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir()
        .join("specdev-gate-registry-it")
        .join(format!("{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// The inlined `profile.yaml` for the test project, with `extra` appended
/// after the required keys.
fn write_profile(root: &Path, extra: &str) {
    fs::write(
        root.join("specs/specdev/profile.yaml"),
        format!("name: test-profile\nlanguage: rust\n{extra}"),
    )
    .unwrap();
}

/// A project with no inlined `profile.yaml` yet.
fn minimal_project(root: &Path) -> LayoutContext {
    fs::create_dir_all(root.join("specs/specdev")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("spec.toml"),
        r#"
spec_version = "1.0"

[project]
name = "gate-registry-fixture"
profile = "test-profile"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    LayoutContext::discover(root, None).expect("discover")
}

fn load_error(layout: &LayoutContext) -> String {
    match Profile::load_inlined(&layout.specdev_root) {
        Ok(_) => panic!("the registry must be refused at load"),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn duplicate_gate_labels_are_refused_at_profile_load() {
    let root = tmp("registry-dup");
    let layout = minimal_project(&root);
    write_profile(
        &root,
        "gates:\n  - label: check\n    scope: module\n    steps: [[cargo, check]]\n  - label: check\n    scope: batch\n    steps: [[cargo, build]]\n",
    );
    let msg = load_error(&layout);
    assert!(msg.contains("declares label `check` twice"), "{msg}");
}

#[test]
fn registry_schema_refuses_empty_steps_and_bad_scope() {
    let root = tmp("registry-schema");
    let layout = minimal_project(&root);
    for bad in [
        "gates:\n  - label: check\n    scope: module\n    steps: []\n",
        "gates:\n  - label: check\n    scope: module\n    steps: [[]]\n",
        "gates:\n  - label: check\n    scope: always\n    steps: [[cargo]]\n",
        "gates:\n  - label: \"has space\"\n    scope: module\n    steps: [[cargo]]\n",
        "gates:\n  - label: check\n    steps: [[cargo]]\n",
        "gates:\n  - label: check\n    scope: module\n    steps: [\"cargo check\"]\n",
    ] {
        write_profile(&root, bad);
        let msg = load_error(&layout);
        assert!(msg.contains("MANIFEST-001"), "{bad}\n{msg}");
    }
}
