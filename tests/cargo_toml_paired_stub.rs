//! `cargo_toml.bin.paired_stub.template_file` file read.
//!
//! The audio-plugin profile's
//! `cargo_toml.bin.paired_stub.template_file` references a template
//! file under the profile dir (`templates/standalone-stub.rs.template`).
//! Bootstrap (a) copies `<profile>/templates/*` to
//! `<target>/specs/specdev/templates/*`, and (b) reads the template at
//! cargo-toml render time, substituting `{project_name}` with the
//! project's crate name to produce the `extra_files` standalone.rs body.
//!
//! Backward-compat: manifest-less projects keep using the
//! legacy `standalone_bin_stub()` literal — same `compile_error!`
//! reminder, slightly different prose.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
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
        "specdev-cargo-toml-paired-stub-{tag}-{}-{}",
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

fn fresh_audio_plugin(tag: &str, project_name: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "audio-plugin".into(),
            language: "rust".into(),
            project_name: project_name.into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn manifest_path(target: &Path) -> PathBuf {
    target.join("specs/specdev/profile.yaml")
}

// ===========================================================================
// 1 — Bootstrap copies templates/* to specs/specdev/templates/*
// ===========================================================================

#[test]
fn bootstrap_copies_paired_stub_template_to_inlined_tree() {
    let (target, _layout) = fresh_audio_plugin("bootstrap-templates", "myfx");
    let inlined = target.join("specs/specdev/templates/standalone-stub.rs.template");
    assert!(
        inlined.is_file(),
        "bootstrap must copy templates/* to specs/specdev/templates/* — missing {}",
        inlined.display()
    );
    // Byte-faithful: the inlined copy equals the plugin-tree source.
    let plugin_src =
        full_plugin().join("profiles/audio-plugin/templates/standalone-stub.rs.template");
    assert_eq!(
        fs::read(&plugin_src).unwrap(),
        fs::read(&inlined).unwrap(),
        "templates/* must be copied byte-faithfully (no substitution)"
    );
}

// ===========================================================================
// 2 — Bootstrap silently skips when profile has no templates/ dir
// ===========================================================================

#[test]
fn bootstrap_skips_templates_when_profile_has_none() {
    // library-api ships no templates/ dir.
    let workspace = tmp_workspace("bootstrap-no-templates");
    let target = workspace.join("proj");
    fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "liblib".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let templates_dir = target.join("specs/specdev/templates");
    assert!(
        !templates_dir.exists(),
        "no templates/ dir in profile → no templates/ in target"
    );
}

// ===========================================================================
// 3 — Manifest path reads template + substitutes {project_name}
// ===========================================================================

#[test]
fn manifest_path_substitutes_project_name_into_paired_stub_body() {
    let (_target, layout) = fresh_audio_plugin("manifest-substitute", "wobble_synth");
    let rendered = render(&layout).unwrap();
    let stub = rendered
        .extra_files
        .iter()
        .find(|(p, _)| p == &PathBuf::from("src/bin/standalone.rs"))
        .expect("audio-plugin must emit standalone.rs stub");
    // Manifest path reads the template, which contains `{project_name}`;
    // substitution replaces it with the project's crate name.
    assert!(
        stub.1.contains("wobble_synth"),
        "stub must mention substituted project_name; got:\n{}",
        stub.1
    );
    assert!(
        !stub.1.contains("{project_name}"),
        "stub must NOT contain unsubstituted token; got:\n{}",
        stub.1
    );
    // Sanity: still has the compile_error! reminder (preserves the
    // friendly fail-fast behavior the legacy literal had).
    assert!(stub.1.contains("compile_error!"));
}

// ===========================================================================
// 4 — Manifest path body matches the template-file body (post-substitution)
// ===========================================================================

#[test]
fn manifest_path_body_matches_template_file_after_substitution() {
    let (target, layout) = fresh_audio_plugin("manifest-body-equals", "myfx");
    let rendered = render(&layout).unwrap();
    let stub = rendered
        .extra_files
        .iter()
        .find(|(p, _)| p == &PathBuf::from("src/bin/standalone.rs"))
        .expect("standalone.rs in extra_files");
    let template_body =
        fs::read_to_string(target.join("specs/specdev/templates/standalone-stub.rs.template"))
            .unwrap();
    let expected = template_body.replace("{project_name}", "myfx");
    assert_eq!(
        stub.1, expected,
        "manifest-path stub body must equal the template file with {{project_name}} → project name"
    );
}

// ===========================================================================
// 5 — Fallback path (manifest absent) uses the legacy standalone_bin_stub
//     literal — different prose, same compile_error! reminder
// ===========================================================================

#[test]
fn fallback_path_uses_legacy_literal_when_manifest_missing() {
    let (target, _initial_layout) = fresh_audio_plugin("fallback-literal", "demo_fx");
    // Strip the manifest (and the templates dir to ensure no accidental
    // read). Fallback path should fire and use the legacy literal.
    fs::remove_file(manifest_path(&target)).unwrap();
    fs::remove_dir_all(target.join("specs/specdev/templates")).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let rendered = render(&layout).unwrap();
    let stub = rendered
        .extra_files
        .iter()
        .find(|(p, _)| p == &PathBuf::from("src/bin/standalone.rs"))
        .expect("fallback must still emit standalone.rs stub");
    // Legacy literal mentions "(F-016 stub)" in its header — the new
    // template uses a more verbose form.
    assert!(
        stub.1.contains("(F-016 stub)"),
        "fallback body should be the legacy standalone_bin_stub literal:\n{}",
        stub.1
    );
    assert!(stub.1.contains("compile_error!"));
}

// ===========================================================================
// 6 — Cargo.toml output is byte-stable (parity preserved across paths,
//     even though the standalone.rs body intentionally drifts)
// ===========================================================================

#[test]
fn parity_cargo_toml_text_manifest_vs_fallback() {
    let (target, layout) = fresh_audio_plugin("parity-cargo-text", "myfx");
    let manifest_text = render(&layout).unwrap().text;

    fs::remove_file(manifest_path(&target)).unwrap();
    fs::remove_dir_all(target.join("specs/specdev/templates")).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let fallback_text = render(&layout).unwrap().text;

    assert_eq!(
        manifest_text, fallback_text,
        "parity: Cargo.toml text is byte-stable across paths even though standalone.rs body migrates"
    );
}

// ===========================================================================
// 7 — Manifest path errors clearly when template_file is missing on disk
// ===========================================================================

#[test]
fn manifest_path_errors_when_template_file_missing() {
    let (target, _layout) = fresh_audio_plugin("missing-template", "myfx");
    // Delete the inlined template — manifest still claims it exists.
    fs::remove_file(target.join("specs/specdev/templates/standalone-stub.rs.template")).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let err = render(&layout).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("standalone-stub.rs.template"),
        "error must name the missing template file:\n{msg}"
    );
}
