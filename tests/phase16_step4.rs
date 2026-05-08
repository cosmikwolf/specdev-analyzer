//! Phase 16 §8.1 step 4 — `skeleton.rs` manifest-driven dispatch.
//!
//! Per doc 21 §3.4: when `specs/specdev/profile.yaml` is present,
//! `gen --skeleton` gates on `skeleton.supported` (instead of the legacy
//! `if profile != "library-api"`) and walks `skeleton.steps` for emission
//! ordering. Step 4 ships the *machinery* — the dispatch + step-walker —
//! while leaving content authoring (firmware-layered / audio-plugin
//! skeleton bodies) for follow-up phases gated on dogfood evidence.
//!
//! Step 4 coverage:
//!   1. Manifest dispatch: library-api (`supported: true`) emits the
//!      expected lib.rs + per-module .rs + Cargo.toml + HANDOFF.md set.
//!   2. Manifest dispatch: firmware-layered / rust-workspace
//!      (`supported: false`) error with `SKEL-001` and the
//!      manifest-vocabulary message (no specific profile name leaked).
//!      audio-plugin (`supported: true`, silence-and-log policy) is
//!      covered separately in `audio_plugin_skeleton_silence_and_log.rs`.
//!   3. Pre-D52 fallback: phase3 firmware-layered (no `profile.yaml`)
//!      errors with `SKEL-001` and the legacy vocabulary (profile name
//!      visible — only the manifest path uses D51-clean vocabulary).
//!   4. Byte-parity: manifest-driven and fallback paths emit identical
//!      artifacts for library-api (lib.rs + every module file + HANDOFF.md
//!      + Cargo.toml).
//!   5. Malformed `profile.yaml` propagates as a load-time error before
//!      skeleton emission begins (no partial artifacts on disk).

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    skeleton,
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
        "specdev-phase16-step4-{tag}-{}-{}",
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

/// Two-module library-api spec authored for step 4 emission tests. Same
/// shape phase13_c uses; duplicated here to keep step 4 hermetic.
fn write_library_two_module_spec(target: &Path) {
    std::fs::write(
        target.join("specs/modules/Types.toml"),
        r#"name = "Types"
visibility = "pub"
description = "Public types module"

[[items]]
kind = "struct"
name = "Frame"
description = "A single frame value."
satisfies = ["REQ-LIB-1"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Api.toml"),
        r#"name = "Api"
visibility = "pub"
description = "Public API surface"

[[items]]
kind = "fn"
name = "process"
description = "Process a frame."
satisfies = ["REQ-LIB-2"]
signature_hint = "fn process(frame: u32) -> u32"
"#,
    )
    .unwrap();
}

fn manifest_path(target: &Path) -> PathBuf {
    target.join("specs/specdev/profile.yaml")
}

// ===========================================================================
// 1 — Manifest dispatch: library-api (supported: true) emits expected files
// ===========================================================================

#[test]
fn manifest_dispatch_library_api_emits_artifacts() {
    let (target, layout) =
        fresh_project(&phase6_plugin(), "library-api", "skel_demo", "lib-emits");
    write_library_two_module_spec(&target);
    assert!(
        manifest_path(&target).is_file(),
        "library-api bootstrap must populate profile.yaml (step 2.5 invariant)"
    );

    let report = skeleton::run_skeleton(&layout).expect("library-api skeleton");
    assert_eq!(report.modules.len(), 2);
    assert!(target.join("Cargo.toml").is_file());
    assert!(target.join("src/lib.rs").is_file());
    assert!(target.join("src/api.rs").is_file());
    assert!(target.join("src/types.rs").is_file());
    assert!(target.join("specs/HANDOFF.md").is_file());
}

// ===========================================================================
// 2 — Manifest dispatch: supported: false shapes error SKEL-001 with
//     manifest vocabulary (no specific profile name leaked)
// ===========================================================================

fn assert_skel_001_manifest_vocab(profile: &str, tag: &str) {
    let (_target, layout) = fresh_project(&phase6_plugin(), profile, "skel_demo", tag);
    let err = skeleton::run_skeleton(&layout).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("SKEL-001"),
        "expected SKEL-001 for {profile}; got {msg}"
    );
    assert!(
        msg.contains("skeleton.supported: false"),
        "manifest path must use shape-vocabulary; got {msg}"
    );
    // D51: never name the specific profile in the manifest-driven message.
    assert!(
        !msg.contains(profile),
        "manifest path must NOT name the specific profile `{profile}` (D51); got {msg}"
    );
}

#[test]
fn manifest_dispatch_firmware_layered_errors_skel_001() {
    assert_skel_001_manifest_vocab("firmware-layered", "fw-skel");
}

#[test]
fn manifest_dispatch_rust_workspace_errors_skel_001() {
    assert_skel_001_manifest_vocab("rust-workspace", "ws-skel");
}

// ===========================================================================
// 3 — Pre-D52 fallback: legacy gate fires when profile.yaml is absent;
//     legacy vocabulary names the profile (this path is unchanged)
// ===========================================================================

#[test]
fn pre_d52_fallback_firmware_layered_errors_legacy_vocab() {
    let (_target, layout) =
        fresh_project(&phase3_plugin(), "firmware-layered", "skel_demo", "fb-fw");
    assert!(
        !layout.specdev_root.join("profile.yaml").exists(),
        "phase3 fixture must NOT carry profile.yaml (test premise)"
    );
    let err = skeleton::run_skeleton(&layout).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("SKEL-001"), "expected SKEL-001; got {msg}");
    // Legacy vocab still names the profile — backward-compat per §6.5.
    assert!(
        msg.contains("library-api only in v1"),
        "fallback message must use legacy vocabulary; got {msg}"
    );
    assert!(
        msg.contains("firmware-layered"),
        "legacy fallback names the profile; got {msg}"
    );
}

// ===========================================================================
// 4 — Byte-parity: manifest path and fallback path emit byte-identical
//     artifacts for library-api
// ===========================================================================

fn read_or_empty(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn manifest_vs_fallback_byte_parity_library_api() {
    // (a) Manifest path: bootstrap library-api with profile.yaml.
    let (target_a, layout_a) =
        fresh_project(&phase6_plugin(), "library-api", "parity_demo", "parity-a");
    write_library_two_module_spec(&target_a);
    skeleton::run_skeleton(&layout_a).expect("manifest-path skeleton");
    let cargo_a = read_or_empty(&target_a.join("Cargo.toml"));
    let lib_a = read_or_empty(&target_a.join("src/lib.rs"));
    let api_a = read_or_empty(&target_a.join("src/api.rs"));
    let types_a = read_or_empty(&target_a.join("src/types.rs"));
    let handoff_a = read_or_empty(&target_a.join("specs/HANDOFF.md"));

    // (b) Fallback path: bootstrap library-api on the same plugin then
    //     remove profile.yaml so the legacy gate fires. Same source spec.
    let (target_b, _layout_b_initial) =
        fresh_project(&phase6_plugin(), "library-api", "parity_demo", "parity-b");
    write_library_two_module_spec(&target_b);
    std::fs::remove_file(target_b.join("specs/specdev/profile.yaml")).unwrap();
    let layout_b = LayoutContext::discover(&target_b, None).unwrap();
    skeleton::run_skeleton(&layout_b).expect("fallback-path skeleton");
    let cargo_b = read_or_empty(&target_b.join("Cargo.toml"));
    let lib_b = read_or_empty(&target_b.join("src/lib.rs"));
    let api_b = read_or_empty(&target_b.join("src/api.rs"));
    let types_b = read_or_empty(&target_b.join("src/types.rs"));
    let handoff_b = read_or_empty(&target_b.join("specs/HANDOFF.md"));

    assert_eq!(cargo_a, cargo_b, "Cargo.toml byte-parity");
    assert_eq!(lib_a, lib_b, "src/lib.rs byte-parity");
    assert_eq!(api_a, api_b, "src/api.rs byte-parity");
    assert_eq!(types_a, types_b, "src/types.rs byte-parity");
    assert_eq!(handoff_a, handoff_b, "HANDOFF.md byte-parity");
}

// ===========================================================================
// 5 — Malformed profile.yaml surfaces as a load-time error from
//     run_skeleton itself, before any artifacts hit disk
// ===========================================================================

#[test]
fn malformed_profile_yaml_errors_before_emission() {
    let (target, _initial_layout) =
        fresh_project(&phase6_plugin(), "library-api", "skel_demo", "bad-yaml");
    write_library_two_module_spec(&target);
    // Overwrite manifest with content that fails schema validation
    // (missing required `name` + `language` fields).
    std::fs::write(
        target.join("specs/specdev/profile.yaml"),
        "garbage: true\n",
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let err = skeleton::run_skeleton(&layout).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("MANIFEST-001"),
        "malformed manifest must surface MANIFEST-001; got {msg}"
    );
    // No skeleton artifacts should have been written.
    assert!(
        !target.join("src/lib.rs").exists(),
        "no lib.rs should be written when manifest load fails"
    );
    assert!(
        !target.join("Cargo.toml").exists()
            || std::fs::read_to_string(target.join("Cargo.toml"))
                .unwrap_or_default()
                .is_empty(),
        "no Cargo.toml should be written when manifest load fails"
    );
}
