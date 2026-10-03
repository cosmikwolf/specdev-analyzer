//! Bootstrap copies a profile's `templates/*` into the project.
//!
//! The templates are the sources of the profile's `paired_files` (config files
//! written once when absent). Bootstrap (a) copies `<profile>/templates/*`
//! byte-faithfully to `<target>/specs/specdev/templates/*`, and (b) copies
//! nothing when the profile ships no `templates/` dir.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-bootstrap-templates-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

fn bootstrap(plugin: &Path, workspace: &Path, profile: &str) -> PathBuf {
    let target = workspace.join("proj");
    fs::create_dir_all(&target).unwrap();
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

#[test]
fn bootstrap_copies_profile_templates_to_inlined_tree() {
    let workspace = tmp_workspace("copies");
    let target = bootstrap(&full_plugin(), &workspace, "audio-plugin");
    let name = "specdev-ci.yml.template";
    let inlined = target.join("specs/specdev/templates").join(name);
    assert!(
        inlined.is_file(),
        "bootstrap must copy templates/* to specs/specdev/templates/* — missing {}",
        inlined.display()
    );
    let plugin_src = full_plugin().join("profiles/audio-plugin/templates").join(name);
    assert_eq!(
        fs::read(&plugin_src).unwrap(),
        fs::read(&inlined).unwrap(),
        "templates/* must be copied byte-faithfully (no substitution)"
    );
}

#[test]
fn bootstrap_skips_templates_when_profile_has_none() {
    // A copy of the test plugin whose library-api profile ships no templates/
    // dir and so declares no paired files.
    let workspace = tmp_workspace("none");
    let plugin = workspace.join("_plugin");
    copy_dir(&full_plugin(), &plugin);
    let profile_dir = plugin.join("profiles/library-api");
    fs::remove_dir_all(profile_dir.join("templates")).unwrap();
    let manifest = profile_dir.join("profile.yaml");
    let text = fs::read_to_string(&manifest).unwrap();
    let kept: Vec<&str> =
        text.lines().filter(|l| !l.starts_with("paired_files:") && !l.contains("template_file:")).collect();
    fs::write(&manifest, kept.join("\n") + "\n").unwrap();

    let target = bootstrap(&plugin, &workspace, "library-api");
    let templates_dir = target.join("specs/specdev/templates");
    assert!(!templates_dir.exists(), "no templates/ dir in profile → no templates/ in target");
}
