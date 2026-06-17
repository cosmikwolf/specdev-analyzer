//! Block `deps.yaml` consumption in `merge_dep`.
//!
//! When a block is inlined, its
//! `codegen/<lang>/deps.yaml` declares Cargo crates the block
//! contributes. `cargo_toml::render` walks every inlined block and
//! merges these contributions into the project's `[dependencies]`
//! section. Closes F-012 + F-017 (blocks no longer require users to
//! mirror their deps in `[[external_deps]]`).
//!
//! Inlined blocks now alter Cargo deps: previously inlined blocks added
//! schemas/reductions/codegen prose but no Cargo deps; now the deps
//! appear automatically.
//!
//! Coverage:
//!   1. Embassy block contributes `embassy-executor` + `embassy-time`.
//!   2. `embassy-stm32` is bridge-owned: stm32h5-rs alone
//!      contributes nothing; the embassy+stm32h5-rs bridge contributes
//!      it with its default chip feature.
//!   3. User authoring wins on conflict (warning surfaces).
//!   4. `recommended_external_deps[]` is NOT auto-injected.
//!   5. Profile-agnostic: works for audio-plugin too (nih_plug).
//!   6. No-op for projects with no inlined blocks.
//!   7. No-op for the manifest-less minimal_plugin fixture (blocks ship
//!      no deps.yaml).

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    inline::run_inline_block,
    layout::LayoutContext,
};

fn minimal_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("minimal_plugin")
        .join("_plugin")
}

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-block-deps-merge-{tag}-{}-{}",
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

fn fresh_project(plugin: &Path, profile: &str, name: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    fs::create_dir_all(&target).unwrap();
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

// ===========================================================================
// 1 — Embassy block contributes embassy-executor + embassy-time
// ===========================================================================

#[test]
fn embassy_block_contributes_executor_and_time() {
    let (_target, layout) = fresh_project(&full_plugin(), "firmware-layered", "demo", "embassy");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    let out = render(&layout).unwrap().text;
    assert!(
        out.contains("embassy-executor"),
        "embassy block must contribute embassy-executor:\n{out}"
    );
    assert!(
        out.contains("embassy-time"),
        "embassy block must contribute embassy-time:\n{out}"
    );
}

// ===========================================================================
// 2 — embassy-stm32 is the bridge's join-product dep, NOT the
//     stm32h5-rs block's. Block alone contributes nothing; inlining the
//     embassy+stm32h5-rs bridge contributes embassy-stm32 with its chip
//     feature.
// ===========================================================================

#[test]
fn stm32h5_rs_block_alone_contributes_no_crates() {
    let (_target, layout) =
        fresh_project(&full_plugin(), "firmware-layered", "demo", "stm32h5");
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    let out = render(&layout).unwrap().text;
    assert!(
        !out.contains("embassy-stm32"),
        "stm32h5-rs alone must NOT contribute embassy-stm32 (bridge-owned):\n{out}"
    );
}

#[test]
fn bridge_contributes_embassy_stm32_with_chip_feature() {
    let (_target, layout) =
        fresh_project(&full_plugin(), "firmware-layered", "demo", "bridge-deps");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    specdev_analyzer::inline::run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs")
        .unwrap();
    let out = render(&layout).unwrap().text;
    assert!(
        out.contains("embassy-stm32"),
        "embassy+stm32h5-rs bridge must contribute embassy-stm32:\n{out}"
    );
    assert!(
        out.contains("stm32h563zi"),
        "bridge's embassy-stm32 must carry its default chip feature:\n{out}"
    );
}

// ===========================================================================
// 3 — User authoring wins on conflict; warning surfaces
// ===========================================================================

#[test]
fn user_external_deps_win_on_version_conflict_with_block() {
    let (target, layout) =
        fresh_project(&full_plugin(), "firmware-layered", "demo", "conflict");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();

    // User authors embassy-executor at a different version in
    // architecture.toml's [[external_deps]] BEFORE the block deps merge.
    // merge_dep is first-insert-wins, and architecture/module deps
    // merge first per the render order — so the user's version stays.
    let arch_path = target.join("specs/architecture.toml");
    let mut arch = fs::read_to_string(&arch_path).unwrap();
    arch.push_str(
        r#"
[[external_deps]]
crate = "embassy-executor"
version = "0.5-user-pin"
source = "crates-io"
"#,
    );
    fs::write(&arch_path, arch).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();

    let rendered = render(&layout).unwrap();
    assert!(
        rendered.text.contains("0.5-user-pin"),
        "user-spec version must win over block contribution:\n{}",
        rendered.text
    );
    assert!(
        !rendered.text.contains(r#"embassy-executor = { version = "0.10""#)
            && !rendered.text.contains(r#"embassy-executor = "0.10""#),
        "block's 0.10 version must NOT appear when user pinned 0.5:\n{}",
        rendered.text
    );
    assert!(
        rendered
            .warnings
            .iter()
            .any(|w| w.contains("embassy-executor")),
        "merge conflict must surface a warning naming embassy-executor; got: {:?}",
        rendered.warnings
    );
}

// ===========================================================================
// 4 — recommended_external_deps[] is NOT auto-injected
// ===========================================================================

#[test]
fn recommended_external_deps_are_not_auto_injected() {
    // stm32h5-rs's deps.yaml lists `cortex-m` and `cortex-m-rt` under
    // `recommended_external_deps`. Those are /spec-design suggestions,
    // not auto-injected dependencies — the user explicitly authors them
    // in `[[external_deps]]` if they want them. They must NOT be
    // injected silently.
    let (target, layout) =
        fresh_project(&full_plugin(), "firmware-layered", "demo", "no-recommend");
    // embassy provides the positive control (its crates[] are non-empty);
    // stm32h5-rs provides the recommended_external_deps under test.
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    // Strip the bootstrapped architecture.toml's [[external_deps]] which
    // ships cortex-m / cortex-m-rt by default — we want to test that
    // ONLY the block's `crates[]` are auto-merged, not the
    // `recommended_external_deps[]`.
    let arch_path = target.join("specs/architecture.toml");
    fs::write(
        &arch_path,
        r#"[project]

[platform]
target_triple = "thumbv8m.main-none-eabihf"
"#,
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let out = render(&layout).unwrap().text;
    // embassy-executor (from embassy's crates[]) should be present.
    assert!(
        out.contains("embassy-executor"),
        "block crates[] must be merged:\n{out}"
    );
    // cortex-m and cortex-m-rt (from recommended_external_deps[]) must
    // NOT be present — they're suggestions, not auto-injections.
    assert!(
        !out.contains("\ncortex-m = ") && !out.contains("\ncortex-m-rt = "),
        "recommended_external_deps[] must NOT auto-inject — found cortex-m or cortex-m-rt:\n{out}"
    );
}

// ===========================================================================
// 5 — Profile-agnostic: audio-plugin profile also picks up block deps
// ===========================================================================

#[test]
fn audio_plugin_block_contributes_nih_plug_via_git_source() {
    let (_target, layout) =
        fresh_project(&full_plugin(), "audio-plugin", "myfx", "audio-nih");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();
    let out = render(&layout).unwrap().text;
    assert!(
        out.contains("nih_plug"),
        "nih-plug block must contribute nih_plug crate:\n{out}"
    );
    assert!(
        out.contains("github.com/robbert-vdh/nih-plug"),
        "nih_plug must be sourced from the upstream git repo:\n{out}"
    );
}

// ===========================================================================
// 6 — No-op when no blocks are inlined
// ===========================================================================

#[test]
fn no_inlined_blocks_no_block_deps_merged() {
    let (_target, layout) = fresh_project(&full_plugin(), "library-api", "liblib", "no-blocks");
    let out = render(&layout).unwrap().text;
    // library-api default architecture has no [[external_deps]] and the
    // user authored no modules — output should be a minimal lib shape
    // without any block-contributed deps. Specifically, no embassy /
    // nih_plug / egui crates appear.
    assert!(!out.contains("embassy-executor"));
    assert!(!out.contains("nih_plug"));
    assert!(!out.contains("\negui = "));
}

// ===========================================================================
// 7 — Backward-compat: manifest-less fixture (minimal_plugin) — blocks
//     ship no deps.yaml, so the merge silently no-ops on those projects.
// ===========================================================================

#[test]
fn manifest_less_fixture_inline_no_block_deps_no_op() {
    assert!(
        !minimal_plugin()
            .join("blocks/embassy/codegen/rust/deps.yaml")
            .exists(),
        "minimal_plugin fixture must NOT ship deps.yaml (test premise)"
    );
    let (_target, layout) =
        fresh_project(&minimal_plugin(), "firmware-layered", "demo", "no-deps-yaml");
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    // The minimal_plugin fixture's embassy block ships no deps.yaml, so
    // the merge contributes nothing. The render should succeed with
    // whatever the architecture.toml's defaults declare — and not gain
    // embassy-executor / embassy-time from block deps.
    let out = render(&layout).unwrap().text;
    assert!(
        !out.contains("embassy-executor") && !out.contains("embassy-time"),
        "no deps.yaml in minimal_plugin fixture → no block-deps merged:\n{out}"
    );
}
