//! Load every authored manifest in the plugin tree (and its full_plugin
//! fixture mirror) via the manifest loaders.
//!
//! The authored content is 10 manifests + 1 paired-stub template. The
//! schemas + loaders + their unit tests already cover the manifest
//! shape. This file adds the integration check that the *actually
//! authored* YAML round-trips against those loaders.
//!
//! The plugin tree itself is outside this crate (`../specdev/...`); we
//! exercise the full_plugin fixture (`tests/specs/full_plugin/_plugin/`)
//! which is kept in lockstep with the plugin tree per existing
//! convention. A separate test asserts the two locations agree
//! byte-for-byte so divergence surfaces immediately.

use std::path::{Path, PathBuf};

use specdev_analyzer::manifest::{
    BindingLocality, BlockDeps, BridgeContract, Profile, ShapeKind,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn live_plugin() -> PathBuf {
    // full_plugin fixture mirrors `<repo>/specdev/`. Walk up to find it.
    // Marker: `.claude-plugin/plugin.json` (the Claude Code plugin manifest).
    let mut cursor = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    loop {
        let candidate = cursor.join("specdev");
        if candidate.is_dir()
            && candidate
                .join(".claude-plugin")
                .join("plugin.json")
                .is_file()
        {
            return candidate;
        }
        match cursor.parent() {
            Some(p) => cursor = p.to_path_buf(),
            None => panic!("could not find live specdev plugin tree from CARGO_MANIFEST_DIR"),
        }
    }
}

const PROFILES: &[&str] = &[
    "library-api",
    "firmware-layered",
    "audio-plugin",
    "rust-workspace",
    "flutter-app",
    "rust-app",
];

// (name, codegen language) — the deps manifest lives at
// blocks/<name>/codegen/<lang>/deps.yaml, so language is part of the row.
// This list is meant to be the full inventory, and it had silently drifted:
// two shipped blocks were missing, so their manifests were unchecked while the
// suite looked complete. Add a row whenever a block ships.
const BLOCKS: &[(&str, &str)] = &[
    ("nih-plug", "rust"),
    ("egui", "rust"),
    ("eframe", "rust"),
    ("embassy", "rust"),
    ("iroh", "rust"),
    ("rtic", "rust"),
    ("stm32h5-rs", "rust"),
    ("flutter-rust-bridge", "dart"),
    ("universal-ble", "dart"),
];

const BRIDGES: &[&str] = &["egui+nih-plug", "embassy+stm32h5-rs"];

// Blocks that deliberately contribute NO deps, each with its reason. A block
// absent from this list must contribute at least one dep in its language's
// vocabulary — `recommended_external_deps` is suggestion-only and does not
// count. Both directions are asserted, so adding a dep to a listed block is
// as loud as dropping one from an unlisted block.
const ZERO_DEP_BLOCKS: &[(&str, &str)] = &[
    // Topology-only: the non-embassy STM32H5 HAL has no usable crates.io
    // release yet, and the production path (`embassy-stm32`) is the
    // embassy+stm32h5-rs bridge's join-product dep, never the block's.
    ("stm32h5-rs", "no publishable HAL crate"),
    // Host-neutral: egui always arrives from a host — `eframe::egui` from the
    // eframe block, `nih_plug_egui::egui` from the egui+nih-plug bridge.
    // Contributing a version here would put a second, independently-versioned
    // egui in the graph alongside the host's, whose types would not unify.
    ("egui", "the host owns the egui version"),
];

// ===========================================================================
// 1 — full_plugin fixture: every manifest loads cleanly
// ===========================================================================

#[test]
fn fixture_profiles_load_cleanly() {
    let root = full_plugin();
    for name in PROFILES {
        let p = root.join("profiles").join(name).join("profile.yaml");
        let m = Profile::load(&p)
            .unwrap_or_else(|e| panic!("profile {name} failed to load from fixture: {e:#}"));
        assert_eq!(m.name, *name, "manifest .name must match dir name for {name}");
        // Each profile must declare the manifest-emission section
        // matching its language.
        match m.language.as_str() {
            "rust" => assert!(m.cargo_toml.is_some(), "{name} must declare cargo_toml"),
            "dart" => assert!(m.pubspec_yaml.is_some(), "{name} must declare pubspec_yaml"),
            other => panic!("profile {name} declares unexpected language `{other}`"),
        }
    }
}

#[test]
fn fixture_block_deps_load_cleanly() {
    let root = full_plugin();
    for (name, lang) in BLOCKS {
        let p = root
            .join("blocks")
            .join(name)
            .join("codegen")
            .join(lang)
            .join("deps.yaml");
        let m = BlockDeps::load(&p)
            .unwrap_or_else(|e| panic!("deps for block {name} failed to load: {e:#}"));
        let contributes = match *lang {
            "dart" => !m.pub_packages.is_empty(),
            _ => !m.crates.is_empty(),
        };
        match ZERO_DEP_BLOCKS.iter().find(|(b, _)| b == name) {
            Some((_, reason)) => assert!(
                !contributes,
                "block {name} declares deps but is listed as contributing none ({reason}); \
                 either drop the dep or remove it from ZERO_DEP_BLOCKS"
            ),
            None => assert!(
                contributes,
                "block {name} should declare at least one {lang} dep (recommended_external_deps is suggestion-only)"
            ),
        }
    }
}

#[test]
fn fixture_bridge_contracts_load_cleanly() {
    let root = full_plugin();
    for name in BRIDGES {
        let p = root.join("bridges").join(name).join("contract.yaml");
        let m = BridgeContract::load(&p)
            .unwrap_or_else(|e| panic!("contract for bridge {name} failed to load: {e:#}"));
        // Both shipped bridges declare a non-empty diagnostic_template.
        assert!(
            !m.diagnostic_template.trim().is_empty(),
            "bridge {name} diagnostic_template must be non-empty"
        );
    }
}

// ===========================================================================
// 2 — Live plugin tree: every manifest loads cleanly (catches drift the
//     fixture wouldn't)
// ===========================================================================

#[test]
fn live_plugin_tree_profiles_load_cleanly() {
    let root = live_plugin();
    for name in PROFILES {
        let p = root.join("profiles").join(name).join("profile.yaml");
        Profile::load(&p)
            .unwrap_or_else(|e| panic!("live profile {name} failed to load: {e:#}"));
    }
}

#[test]
fn live_plugin_tree_block_deps_load_cleanly() {
    let root = live_plugin();
    for (name, lang) in BLOCKS {
        let p = root
            .join("blocks")
            .join(name)
            .join("codegen")
            .join(lang)
            .join("deps.yaml");
        BlockDeps::load(&p).unwrap_or_else(|e| panic!("live deps for {name} failed: {e:#}"));
    }
}

#[test]
fn live_plugin_tree_bridge_contracts_load_cleanly() {
    let root = live_plugin();
    for name in BRIDGES {
        let p = root.join("bridges").join(name).join("contract.yaml");
        BridgeContract::load(&p)
            .unwrap_or_else(|e| panic!("live contract for {name} failed: {e:#}"));
    }
}

#[test]
fn live_plugin_tree_bridge_deps_load_cleanly() {
    // Bridges may ship a codegen/<lang>/deps.yaml for join-product
    // crates (same manifest shape as block deps). `load` returns the
    // default for bridges that ship none, so this covers both cases.
    let root = live_plugin();
    for name in BRIDGES {
        let p = root
            .join("bridges")
            .join(name)
            .join("codegen")
            .join("rust")
            .join("deps.yaml");
        BlockDeps::load(&p)
            .unwrap_or_else(|e| panic!("live bridge deps for {name} failed: {e:#}"));
    }
}

// ===========================================================================
// 3 — Fixture ↔ live tree byte-equality (catches sync drift)
// ===========================================================================

#[test]
fn fixture_matches_live_plugin_tree_byte_for_byte() {
    let fixture_root = full_plugin();
    let live_root = live_plugin();
    let mut pairs: Vec<(PathBuf, PathBuf, &str)> = Vec::new();
    for name in PROFILES {
        let rel = format!("profiles/{name}/profile.yaml");
        pairs.push((live_root.join(&rel), fixture_root.join(&rel), "profile"));
    }
    for (name, lang) in BLOCKS {
        let rel = format!("blocks/{name}/codegen/{lang}/deps.yaml");
        pairs.push((live_root.join(&rel), fixture_root.join(&rel), "deps"));
    }
    for name in BRIDGES {
        let rel = format!("bridges/{name}/contract.yaml");
        pairs.push((live_root.join(&rel), fixture_root.join(&rel), "contract"));
    }
    // Bridge deps.yaml joins the byte-equality set when the live
    // bridge ships one (not all bridges contribute deps).
    for name in BRIDGES {
        let rel = format!("bridges/{name}/codegen/rust/deps.yaml");
        if live_root.join(&rel).is_file() {
            pairs.push((live_root.join(&rel), fixture_root.join(&rel), "bridge deps"));
        }
    }
    pairs.push((
        live_root.join("profiles/audio-plugin/templates/standalone-stub.rs.template"),
        fixture_root.join("profiles/audio-plugin/templates/standalone-stub.rs.template"),
        "audio-plugin paired-stub template",
    ));

    for (live, fixture, label) in pairs {
        let live_bytes = std::fs::read(&live)
            .unwrap_or_else(|e| panic!("read {} ({label}): {e}", live.display()));
        let fixture_bytes = std::fs::read(&fixture)
            .unwrap_or_else(|e| panic!("read {} ({label}): {e}", fixture.display()));
        assert_eq!(
            live_bytes,
            fixture_bytes,
            "{label} drift between live plugin tree and full_plugin fixture:\n  live: {}\n  fixture: {}",
            live.display(),
            fixture.display()
        );
    }
}

// ===========================================================================
// 4 — Spot-check key field values to catch over-broad authoring mistakes
// ===========================================================================

#[test]
fn fixture_audio_plugin_paired_stub_template_file_exists() {
    let root = full_plugin();
    let manifest = Profile::load(&root.join("profiles/audio-plugin/profile.yaml")).unwrap();
    let cargo = manifest.cargo_toml.unwrap();
    let bin = cargo.bin.expect("audio-plugin bin section present");
    let stub = bin.paired_stub.expect("audio-plugin paired_stub present");
    let stub_path = root.join("profiles/audio-plugin").join(&stub.template_file);
    assert!(
        stub_path.is_file(),
        "audio-plugin paired_stub.template_file must exist at {}",
        stub_path.display()
    );
    let body = std::fs::read_to_string(&stub_path).unwrap();
    assert!(body.contains("{project_name}"), "stub must use {{project_name}} token");
    assert!(body.contains("compile_error!"), "stub must include the friendly reminder");
}

#[test]
fn fixture_rust_workspace_has_workspace_shape_and_no_module_surface() {
    let root = full_plugin();
    let manifest = Profile::load(&root.join("profiles/rust-workspace/profile.yaml")).unwrap();
    let cargo = manifest.cargo_toml.unwrap();
    assert_eq!(cargo.shape, ShapeKind::Workspace);
    let surf = manifest.surface.unwrap();
    assert!(!surf.has_modules, "rust-workspace surface.has_modules must be false (F-009)");
    assert!(!surf.has_requirements);
}

#[test]
fn fixture_egui_nih_plug_bridge_locality_is_plugin_shell_unique() {
    let root = full_plugin();
    let m = BridgeContract::load(&root.join("bridges/egui+nih-plug/contract.yaml")).unwrap();
    assert_eq!(m.binding_locality, BindingLocality::PluginShellUnique);
}

#[test]
fn fixture_embassy_stm32_bridge_locality_is_endpoint_module() {
    let root = full_plugin();
    let m = BridgeContract::load(&root.join("bridges/embassy+stm32h5-rs/contract.yaml")).unwrap();
    assert_eq!(m.binding_locality, BindingLocality::EndpointModule);
}
