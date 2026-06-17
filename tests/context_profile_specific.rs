//! `context.rs` manifest-driven dispatch.
//!
//! When `specs/specdev/profile.yaml` is present,
//! `context --module <M>`'s `profile_specific` field is populated by
//! walking `context.context_subcommand_extras`. Two extra kinds in v1:
//!   * `from_attr` (no explicit `kind`): read named attr off the Module
//!     entity, emit as a string field.
//!   * `kind: collect_local_entities_by_kind` + `kinds: [<EntityKind>...]`:
//!     walk locally-owned entities, group by kind suffix into a sorted
//!     YAML mapping (lowercased + 's' suffix matching the legacy keys).
//!
//! Coverage:
//!   1. Manifest dispatch: library-api emits `visibility` (matches the
//!      legacy hardcoded library-api arm).
//!   2. Manifest dispatch: firmware-layered emits `layer` +
//!      `hardware_resources.{peripherals,pins}` (byte-parity with the
//!      legacy firmware-layered arm).
//!   3. Manifest dispatch: audio-plugin emits `layer` (NEW BEHAVIOR —
//!      audio-plugin had no legacy arm; the manifest authors the `layer`
//!      lookup).
//!   4. Manifest dispatch: rust-workspace emits an empty
//!      `profile_specific` (manifest's extras are []).
//!   5. Manifest-less fallback: minimal_plugin firmware-layered (no
//!      profile.yaml) keeps the legacy hardcoded arm — `layer` still emits.
//!   6. Manifest path with no entity match: empty
//!      `hardware_resources.peripherals` + `hardware_resources.pins`
//!      causes the field to be omitted (current invariant preserved).
//!   7. Malformed `profile.yaml` propagates as MANIFEST-001 from
//!      `run_context` itself.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    context::{self, ContextReport},
    inline::run_inline_block,
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
        "specdev-context-profile-specific-{tag}-{}-{}",
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

fn fresh_project(plugin: &Path, profile: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
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
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn run(layout: &LayoutContext, module: &str) -> ContextReport {
    context::run_context(layout, module).expect("context cone")
}

// ===========================================================================
// 1 — Manifest dispatch: library-api emits `visibility` from manifest extras
// ===========================================================================

#[test]
fn manifest_dispatch_library_api_emits_visibility() {
    let (target, layout) = fresh_project(&full_plugin(), "library-api", "lib-vis");
    std::fs::write(
        target.join("specs/modules/Api.toml"),
        "name = \"Api\"\nvisibility = \"pub\"\ndescription = \"public api\"\n",
    )
    .unwrap();

    let r = run(&layout, "Api");
    assert_eq!(
        r.profile_specific.get("visibility"),
        Some(&serde_yaml::Value::String("pub".into())),
    );
    // No firmware-shaped fields leak through.
    assert!(!r.profile_specific.contains_key("layer"));
    assert!(!r.profile_specific.contains_key("hardware_resources"));
}

// ===========================================================================
// 2 — Manifest dispatch: firmware-layered emits layer + hardware_resources
//     (byte-parity with the legacy hardcoded arm)
// ===========================================================================

#[test]
fn manifest_dispatch_firmware_layered_emits_layer_and_hardware() {
    let (target, layout) = fresh_project(&full_plugin(), "firmware-layered", "fw-hw");
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Uart.toml"),
        r#"name = "Uart"
layer = "driver"

[[stm32h5.peripherals]]
name = "uart_dev"
kind = "USART"
instance = 3

[[stm32h5.pins]]
name = "tx_pin"
port = "B"
pin = 10
af = 7
"#,
    )
    .unwrap();

    let r = run(&layout, "Uart");
    assert_eq!(
        r.profile_specific.get("layer"),
        Some(&serde_yaml::Value::String("driver".into())),
    );
    let hw = r
        .profile_specific
        .get("hardware_resources")
        .expect("hardware_resources expected");
    let hw_map = hw.as_mapping().expect("hardware_resources is a mapping");
    let peripherals = hw_map
        .get(serde_yaml::Value::String("peripherals".into()))
        .expect("peripherals key");
    let peripheral_names: Vec<&str> = peripherals
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        peripheral_names.iter().any(|n| n.contains("uart_dev")),
        "expected uart_dev in peripherals; got {peripheral_names:?}"
    );
    let pins = hw_map
        .get(serde_yaml::Value::String("pins".into()))
        .expect("pins key");
    let pin_names: Vec<&str> = pins
        .as_sequence()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        pin_names.iter().any(|n| n.contains("tx_pin")),
        "expected tx_pin in pins; got {pin_names:?}"
    );
}

// ===========================================================================
// 3 — Manifest dispatch: audio-plugin emits `layer` (NEW BEHAVIOR — the
//     manifest authors the `layer` lookup; there was no legacy arm)
// ===========================================================================

#[test]
fn manifest_dispatch_audio_plugin_emits_layer() {
    let (target, layout) = fresh_project(&full_plugin(), "audio-plugin", "ap-layer");
    std::fs::write(
        target.join("specs/modules/Dsp.toml"),
        "name = \"Dsp\"\nlayer = \"dsp\"\ndescription = \"signal processing\"\n",
    )
    .unwrap();

    let r = run(&layout, "Dsp");
    assert_eq!(
        r.profile_specific.get("layer"),
        Some(&serde_yaml::Value::String("dsp".into())),
        "audio-plugin manifest extras declare `layer` from_attr; emit it"
    );
    // Audio-plugin manifest declares no hardware_resources collector.
    assert!(!r.profile_specific.contains_key("hardware_resources"));
}

// ===========================================================================
// 4 — Manifest dispatch: rust-workspace emits empty profile_specific
//     (manifest's extras list is [])
// ===========================================================================

#[test]
fn manifest_dispatch_rust_workspace_emits_empty_profile_specific() {
    // rust-workspace bootstrap sets surface.has_modules = false; the
    // workspace project has no per-module surface. The context
    // subcommand returns module-not-found rather than running the cone,
    // but its profile_specific is still empty either way.
    let (_target, layout) = fresh_project(&full_plugin(), "rust-workspace", "ws-empty");
    let r = run(&layout, "AnyModule");
    assert!(
        r.profile_specific.is_empty(),
        "rust-workspace manifest declares empty extras; profile_specific should be empty: {:?}",
        r.profile_specific
    );
}

// ===========================================================================
// 5 — Manifest-less fallback: firmware-layered (no profile.yaml) preserves
//     the legacy hardcoded arm
// ===========================================================================

#[test]
fn manifest_less_fallback_firmware_layered_emits_layer() {
    // Use full_plugin (which has the modern module schema with `layer`)
    // but delete profile.yaml after bootstrap to exercise the legacy gate.
    // The minimal_plugin fixture doesn't ship the `layer` field in its
    // module schema, so we can't use it for behavior tests that depend on
    // extracted attrs.
    let (target, _initial_layout) =
        fresh_project(&full_plugin(), "firmware-layered", "fb-fw");
    std::fs::remove_file(target.join("specs/specdev/profile.yaml")).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    assert!(
        !layout.specdev_root.join("profile.yaml").exists(),
        "manifest must be removed for fallback test"
    );
    std::fs::write(
        target.join("specs/modules/Plain.toml"),
        "name = \"Plain\"\nlayer = \"application\"\n",
    )
    .unwrap();
    let r = run(&layout, "Plain");
    assert_eq!(
        r.profile_specific.get("layer"),
        Some(&serde_yaml::Value::String("application".into())),
        "fallback path must still emit layer for firmware-layered"
    );
}

// ===========================================================================
// 6 — Manifest path with no peripheral / pin entities: hardware_resources
//     field is omitted (preserves the legacy emit-only-when-nonempty rule)
// ===========================================================================

#[test]
fn manifest_dispatch_firmware_no_hardware_omits_field() {
    let (target, layout) = fresh_project(&full_plugin(), "firmware-layered", "fw-bare");
    std::fs::write(
        target.join("specs/modules/Bare.toml"),
        "name = \"Bare\"\nlayer = \"application\"\n",
    )
    .unwrap();
    let r = run(&layout, "Bare");
    assert_eq!(
        r.profile_specific.get("layer"),
        Some(&serde_yaml::Value::String("application".into())),
    );
    assert!(
        !r.profile_specific.contains_key("hardware_resources"),
        "no peripherals or pins → omit hardware_resources field"
    );
}

// ===========================================================================
// 7 — Malformed profile.yaml propagates as MANIFEST-001 from run_context
// ===========================================================================

#[test]
fn malformed_profile_yaml_errors_manifest_001() {
    let (target, _initial) = fresh_project(&full_plugin(), "library-api", "bad-yaml");
    std::fs::write(
        target.join("specs/modules/Api.toml"),
        "name = \"Api\"\nvisibility = \"pub\"\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/specdev/profile.yaml"),
        "garbage: true\n",
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let err = context::run_context(&layout, "Api").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("MANIFEST-001"),
        "malformed profile.yaml must surface MANIFEST-001 from run_context; got {msg}"
    );
}
