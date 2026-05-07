//! Phase 16 §8.1 step 3a — `cargo_toml.rs` shape-driven dispatch.
//!
//! Per doc 21 §3.4: when `specs/specdev/profile.yaml` is present, the
//! Cargo.toml renderer dispatches off `cargo_toml.shape` instead of
//! `[project].profile`. Step 3a is **pure routing** — per-shape render
//! functions stay unchanged, output is byte-for-byte identical.
//!
//! Step 3a coverage:
//!   1. Manifest-driven dispatch routes each shape to the right renderer
//!      (Library → render_library, Binary → render_firmware,
//!      Plugin → render_audio_plugin).
//!   2. `shape: workspace` without a `[workspace]` table errors with the
//!      shape-vocabulary diagnostic (no profile name in the message).
//!   3. Backward-compat fallback fires when profile.yaml is absent
//!      (pre-D52 projects bootstrapped before step 2.5).
//!   4. Manifest-driven and fallback paths produce **byte-identical**
//!      output for every shipped profile (the parity gate per doc 21
//!      §8.2).

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
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
        "specdev-phase16-step3a-{tag}-{}-{}",
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

fn manifest_path(target: &Path) -> PathBuf {
    target.join("specs/specdev/profile.yaml")
}

// ===========================================================================
// 1 — Manifest-driven routing: shape=Library → library shape output
// ===========================================================================

#[test]
fn manifest_dispatch_library_shape_renders_library() {
    let (target, layout) = fresh_project(&phase6_plugin(), "library-api", "liblib", "lib-shape");
    assert!(
        manifest_path(&target).is_file(),
        "library-api bootstrap must populate profile.yaml (step 2.5 invariant)"
    );
    let out = render(&layout).unwrap().text;
    // Library shape: `[lib]` section, no `[[bin]]` row.
    assert!(out.contains("[lib]"), "library shape must emit [lib]:\n{out}");
    assert!(
        !out.contains("[[bin]]"),
        "library shape must NOT emit [[bin]]:\n{out}"
    );
}

// ===========================================================================
// 2 — Manifest-driven routing: shape=Binary → firmware-layered shape output
// ===========================================================================

#[test]
fn manifest_dispatch_binary_shape_renders_firmware() {
    let (target, layout) = fresh_project(&phase6_plugin(), "firmware-layered", "demo", "bin-shape");
    assert!(manifest_path(&target).is_file());
    let out = render(&layout).unwrap().text;
    // Binary shape: `[[bin]]` row with name=project_name + path=src/main.rs.
    assert!(out.contains("[[bin]]"), "binary shape must emit [[bin]]:\n{out}");
    assert!(
        out.contains(r#"name = "demo""#),
        "binary shape must emit `name = \"<project>\"`:\n{out}"
    );
    assert!(
        out.contains(r#"path = "src/main.rs""#),
        "binary shape must emit src/main.rs:\n{out}"
    );
}

// ===========================================================================
// 3 — Manifest-driven routing: shape=Plugin → audio-plugin shape output
// ===========================================================================

#[test]
fn manifest_dispatch_plugin_shape_renders_audio_plugin() {
    let (target, layout) = fresh_project(&phase6_plugin(), "audio-plugin", "myfx", "plugin-shape");
    assert!(manifest_path(&target).is_file());
    let out = render(&layout).unwrap().text;
    // Plugin shape: `[lib]` + `crate-type = ["cdylib"]`.
    assert!(out.contains("[lib]"), "plugin shape must emit [lib]:\n{out}");
    assert!(
        out.contains(r#"crate-type = ["cdylib"]"#),
        "plugin shape must emit cdylib crate-type:\n{out}"
    );
}

// ===========================================================================
// 4 — Manifest-driven routing: shape=Workspace without [workspace] errors
//     with a shape-vocabulary diagnostic (no profile name leaked)
// ===========================================================================

#[test]
fn manifest_dispatch_workspace_shape_without_table_errors_with_shape_vocabulary() {
    let (target, _initial_layout) = fresh_project(
        &phase6_plugin(),
        "rust-workspace",
        "broken",
        "ws-no-table",
    );
    // Strip [workspace] from spec.toml — manifest still claims
    // shape: workspace, so the dispatch must error with a shape-aware
    // message.
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
    let err = render(&layout).unwrap_err();
    let msg = format!("{err:#}");
    // Per D51: error vocabulary names the SHAPE and the missing
    // structural signal, never the specific profile name.
    assert!(
        msg.contains("shape: workspace"),
        "error must reference shape vocabulary:\n{msg}"
    );
    assert!(
        msg.contains("[workspace]"),
        "error must reference the missing [workspace] table:\n{msg}"
    );
    assert!(
        !msg.contains("rust-workspace"),
        "error must NOT name the specific profile (D51):\n{msg}"
    );
}

// ===========================================================================
// 5 — Backward-compat fallback: missing profile.yaml routes via profile name
// ===========================================================================

#[test]
fn backward_compat_fallback_when_manifest_missing() {
    // phase3 fixture has firmware-layered without profile.yaml — the
    // pre-D52 baseline. render() must fall back to the profile-name
    // match and produce the firmware-layered binary shape.
    assert!(
        !phase3_plugin()
            .join("profiles/firmware-layered/profile.yaml")
            .exists(),
        "phase3 fixture must NOT ship profile.yaml (test premise)"
    );
    let (target, layout) =
        fresh_project(&phase3_plugin(), "firmware-layered", "demo", "fb-firmware");
    assert!(
        !manifest_path(&target).exists(),
        "phase3 bootstrap must NOT have written profile.yaml"
    );
    let out = render(&layout).unwrap().text;
    assert!(out.contains("[[bin]]"), "fallback firmware shape must emit [[bin]]:\n{out}");
    assert!(
        out.contains(r#"path = "src/main.rs""#),
        "fallback firmware shape must emit src/main.rs:\n{out}"
    );
}

// ===========================================================================
// 6 — Byte-for-byte parity: manifest-driven path matches fallback path for
//     each shipped shape. The parity gate per doc 21 §8.2.
// ===========================================================================

#[test]
fn parity_manifest_vs_fallback_library_api() {
    let (target, layout) =
        fresh_project(&phase6_plugin(), "library-api", "liblib", "parity-lib");
    let manifest_out = render(&layout).unwrap().text;
    // Now strip the manifest and re-render via fallback. Both paths
    // call render_library(); output must be byte-identical.
    std::fs::remove_file(manifest_path(&target)).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let fallback_out = render(&layout).unwrap().text;
    assert_eq!(
        manifest_out, fallback_out,
        "manifest-driven and fallback paths must produce byte-identical Cargo.toml for library-api"
    );
}

#[test]
fn parity_manifest_vs_fallback_firmware_layered() {
    let (target, layout) =
        fresh_project(&phase6_plugin(), "firmware-layered", "demo", "parity-fw");
    let manifest_out = render(&layout).unwrap().text;
    std::fs::remove_file(manifest_path(&target)).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let fallback_out = render(&layout).unwrap().text;
    assert_eq!(
        manifest_out, fallback_out,
        "manifest-driven and fallback paths must produce byte-identical Cargo.toml for firmware-layered"
    );
}

#[test]
fn parity_manifest_vs_fallback_audio_plugin() {
    let (target, layout) =
        fresh_project(&phase6_plugin(), "audio-plugin", "myfx", "parity-ap");
    let manifest_out = render(&layout).unwrap().text;
    std::fs::remove_file(manifest_path(&target)).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let fallback_out = render(&layout).unwrap().text;
    assert_eq!(
        manifest_out, fallback_out,
        "manifest-driven and fallback paths must produce byte-identical Cargo.toml for audio-plugin"
    );
}

// ===========================================================================
// 7 — Workspace short-circuit (cargo_toml.rs:111) is unaffected by manifest
//     dispatch: a project with `[workspace]` set in spec.toml hits the
//     workspace-root render path regardless of profile.yaml content.
// ===========================================================================

#[test]
fn workspace_short_circuit_takes_precedence_over_manifest_dispatch() {
    let (_target, layout) = fresh_project(
        &phase6_plugin(),
        "rust-workspace",
        "demo-ws",
        "ws-short-circuit",
    );
    // Default rust-workspace bootstrap declares [workspace] in
    // spec.toml.template, so the workspace-root path fires first.
    // Output should be the [workspace] block, NOT a per-shape body.
    let out = render(&layout).unwrap().text;
    assert!(out.starts_with("[workspace]"), "expected workspace-root output:\n{out}");
}
