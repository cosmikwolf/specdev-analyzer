//! Load every authored manifest in the plugin tree (and its full_plugin
//! fixture) via the manifest loaders.
//!
//! The schemas + loaders + their unit tests already cover the manifest
//! shape. This file adds the integration check that the *actually
//! authored* YAML round-trips against those loaders.
//!
//! Two rules govern everything here, and both exist because their
//! predecessors were hand-written lists that rotted:
//!
//! 1. **Inventories are discovered, never listed.** Which profiles,
//!    blocks and bridges exist is read off the directory tree, so a
//!    newly shipped unit is checked on the next run rather than when
//!    someone remembers to add a row.
//! 2. **The fixture is a deliberate subset, and every file in it must be
//!    current.** The plugin tree itself is outside this crate
//!    (`../specdev/...`). The fixture at `tests/specs/full_plugin/_plugin/`
//!    copies part of it — no `reference/` crates, not every block or
//!    bridge. `every_fixture_file_matches_the_live_plugin_tree` walks
//!    the fixture and asserts each file it holds is byte-identical to
//!    the live tree, so a stale copy surfaces immediately while the
//!    subset stays an authoring choice.
//!
//! Never hand-edit a file under the fixture: edit the live plugin tree
//! and re-copy, or the walk reports it as drift.

use std::path::{Path, PathBuf};

use specdev_analyzer::manifest::{BindingLocality, BlockDeps, BridgeContract, Profile, ShapeKind};

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

// ---------------------------------------------------------------------------
// Unit inventories are DISCOVERED, never listed.
//
// These were three hand-written `const` arrays. Every one of them had rotted,
// and the rot was invisible because a short list simply checks fewer things
// while the suite still reports a full pass. Measured when this was replaced:
// `PROFILES` 7 of 7 (correct), `BLOCKS` **10 of 14** (`egui-plot`,
// `elf-dwarf`, `itm`, `probe-rs` unchecked) and `BRIDGES` **2 of 5**
// (`egui+egui-plot`, `elf-dwarf+probe-rs`, `itm+probe-rs` unchecked).
//
// The asymmetry that matters: a missing row in an *inclusion* list fails
// silently forever, so the guard degrades exactly where a new unit needs it.
// Discovery inverts that — ship a unit and it is checked on the next run.
// ---------------------------------------------------------------------------

/// Immediate subdirectories of `<root>/<kind>`, sorted for determinism.
/// `read_dir` order is arbitrary, so a failure message would otherwise vary
/// run to run.
fn units_in(root: &Path, kind: &str) -> Vec<String> {
    let dir = root.join(kind);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry"))
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert!(
        !names.is_empty(),
        "discovered zero units under {} — a discovery bug reads as a clean pass, \
         so this asserts rather than vacuously succeeding",
        dir.display()
    );
    names
}

fn profiles_in(root: &Path) -> Vec<String> {
    units_in(root, "profiles")
}

fn bridges_in(root: &Path) -> Vec<String> {
    units_in(root, "bridges")
}

/// `(block, codegen language)` for every `blocks/<n>/codegen/<lang>/deps.yaml`
/// that exists. Language is part of the row because the deps manifest lives
/// under it, and a block may in principle ship more than one.
fn blocks_in(root: &Path) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for name in units_in(root, "blocks") {
        let codegen = root.join("blocks").join(&name).join("codegen");
        if !codegen.is_dir() {
            continue;
        }
        let mut langs: Vec<String> = std::fs::read_dir(&codegen)
            .unwrap_or_else(|e| panic!("read {}: {e}", codegen.display()))
            .map(|e| e.expect("dir entry"))
            .filter(|e| e.path().join("deps.yaml").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        langs.sort();
        for lang in langs {
            rows.push((name.clone(), lang));
        }
    }
    assert!(
        !rows.is_empty(),
        "discovered zero block deps under {}",
        root.display()
    );
    rows
}

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
    for name in &profiles_in(&root) {
        let p = root.join("profiles").join(name).join("profile.yaml");
        let m = Profile::load(&p)
            .unwrap_or_else(|e| panic!("profile {name} failed to load from fixture: {e:#}"));
        assert_eq!(
            &m.name, name,
            "manifest .name must match dir name for {name}"
        );
        // Each profile must declare the manifest-emission section
        // matching its language.
        match m.language.as_str() {
            "rust" => assert!(m.cargo_toml.is_some(), "{name} must declare cargo_toml"),
            "dart" => assert!(m.pubspec_yaml.is_some(), "{name} must declare pubspec_yaml"),
            "typescript" => {
                assert!(m.package_json.is_some(), "{name} must declare package_json")
            }
            other => panic!("profile {name} declares unexpected language `{other}`"),
        }
    }
}

#[test]
fn fixture_block_deps_load_cleanly() {
    let root = full_plugin();
    for (name, lang) in &blocks_in(&root) {
        let p = root
            .join("blocks")
            .join(name)
            .join("codegen")
            .join(lang)
            .join("deps.yaml");
        BlockDeps::load(&p)
            .unwrap_or_else(|e| panic!("deps for block {name} failed to load: {e:#}"));
    }
}

#[test]
fn fixture_bridge_contracts_load_cleanly() {
    let root = full_plugin();
    for name in &bridges_in(&root) {
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
    for name in &profiles_in(&root) {
        let p = root.join("profiles").join(name).join("profile.yaml");
        Profile::load(&p).unwrap_or_else(|e| panic!("live profile {name} failed to load: {e:#}"));
    }
}

#[test]
fn live_plugin_tree_block_deps_load_cleanly() {
    let root = live_plugin();
    for (name, lang) in &blocks_in(&root) {
        let p = root
            .join("blocks")
            .join(name)
            .join("codegen")
            .join(lang)
            .join("deps.yaml");
        let m =
            BlockDeps::load(&p).unwrap_or_else(|e| panic!("live deps for {name} failed: {e:#}"));

        // The zero-dep rule is asserted HERE, against the live tree, not
        // against the fixture. The fixture is a subset — it omits several
        // shipped blocks — so running this over the fixture silently exempts
        // exactly the blocks nobody copied in. Measured when this moved: the
        // fixture holds 10 blocks and the live tree ships 14, so four blocks
        // were escaping the rule entirely.
        let contributes = match lang.as_str() {
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
fn live_plugin_tree_bridge_contracts_load_cleanly() {
    let root = live_plugin();
    for name in &bridges_in(&root) {
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
    for name in &bridges_in(&root) {
        let p = root
            .join("bridges")
            .join(name)
            .join("codegen")
            .join("rust")
            .join("deps.yaml");
        BlockDeps::load(&p).unwrap_or_else(|e| panic!("live bridge deps for {name} failed: {e:#}"));
    }
}

// ===========================================================================
// 3 — Fixture ↔ live tree byte-equality (catches sync drift)
// ===========================================================================

/// Recursively collect every file under `dir`, as paths relative to it.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(base: &Path, cur: &Path, out: &mut Vec<PathBuf>) {
        for entry in
            std::fs::read_dir(cur).unwrap_or_else(|e| panic!("read {}: {e}", cur.display()))
        {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(base, &path, out);
            } else if path.file_name().is_some_and(|n| n == ".DS_Store") {
                // Finder litter, not fixture content. Without this skip, one
                // visit to the fixture in Finder turns this test red with
                // "no longer exists in the live plugin tree" — a message
                // about upstream deletion, for a file the fixture never
                // meant to carry. The plugin repo ignores `.DS_Store`; this
                // repo's `.gitignore` now does too, but an untracked one is
                // still on disk and still gets walked.
                continue;
            } else {
                out.push(path.strip_prefix(base).expect("under base").to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Every file the fixture carries must be byte-identical to the live plugin
/// tree.
///
/// This WALKS the fixture rather than comparing an enumerated list of pairs.
/// The list is what failed: `architecture.toml.template` — the file carrying
/// every profile's crate pins — was simply never added to it, so it drifted
/// unguarded until an emission test was found asserting a version the plugin
/// had stopped shipping. Appending one more line to a list only fixes the
/// entry you already know about.
///
/// The direction is deliberate. The fixture is a **subset** of the live tree
/// by design: it carries no `reference/` crates, omits several blocks and
/// bridges outright, and holds only part of `languages/`. Walking the *live*
/// tree would therefore demand the fixture mirror everything, which is not
/// what it is for. Walking the *fixture* says the right thing instead —
/// "whatever you chose to copy in here must be current" — and it needs no
/// list, so a file added to the fixture tomorrow is guarded on the next run.
///
/// `tests/README.md` documents the subset rule; this test enforces it.
#[test]
fn every_fixture_file_matches_the_live_plugin_tree() {
    let fixture_root = full_plugin();
    let live_root = live_plugin();

    let files = files_under(&fixture_root);
    assert!(
        !files.is_empty(),
        "walked zero files under {} — a walk bug reads as a clean pass",
        fixture_root.display()
    );

    let mut stale = Vec::new();
    let mut absent = Vec::new();
    for rel in &files {
        let fixture = fixture_root.join(rel);
        let live = live_root.join(rel);
        if !live.is_file() {
            absent.push(rel.display().to_string());
            continue;
        }
        let fixture_bytes = std::fs::read(&fixture).expect("read fixture file");
        let live_bytes = std::fs::read(&live).expect("read live file");
        if fixture_bytes != live_bytes {
            stale.push(rel.display().to_string());
        }
    }

    assert!(
        absent.is_empty(),
        "{} fixture file(s) no longer exist in the live plugin tree — the unit was \
         renamed or deleted upstream and the fixture still carries it:\n  {}",
        absent.len(),
        absent.join("\n  ")
    );
    assert!(
        stale.is_empty(),
        "{} fixture file(s) drifted from the live plugin tree. The fixture is the \
         input under test, so a stale copy certifies behaviour against content the \
         plugin no longer ships. Re-copy each from {}:\n  {}",
        stale.len(),
        live_root.display(),
        stale.join("\n  ")
    );
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
    assert!(
        body.contains("{project_name}"),
        "stub must use {{project_name}} token"
    );
    assert!(
        body.contains("compile_error!"),
        "stub must include the friendly reminder"
    );
}

#[test]
fn fixture_rust_workspace_has_workspace_shape_and_no_module_surface() {
    let root = full_plugin();
    let manifest = Profile::load(&root.join("profiles/rust-workspace/profile.yaml")).unwrap();
    let cargo = manifest.cargo_toml.unwrap();
    assert_eq!(cargo.shape, ShapeKind::Workspace);
    let surf = manifest.surface.unwrap();
    assert!(
        !surf.has_modules,
        "rust-workspace surface.has_modules must be false (F-009)"
    );
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
