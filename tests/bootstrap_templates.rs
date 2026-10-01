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

// ===========================================================================
// 4 — Manifest path body matches the template-file body (post-substitution)
// ===========================================================================

// ===========================================================================
// 5 — Fallback path (manifest absent) uses the legacy standalone_bin_stub
//     literal — different prose, same compile_error! reminder
// ===========================================================================

// ===========================================================================
// 6 — Cargo.toml output is byte-stable (parity preserved across paths,
//     even though the standalone.rs body intentionally drifts)
// ===========================================================================

// ===========================================================================
// 7 — Manifest path errors clearly when template_file is missing on disk
// ===========================================================================
