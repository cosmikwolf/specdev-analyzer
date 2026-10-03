//! `specdev-analyzer migrate` subcommand.
//!
//! Pre-manifest projects (bootstrapped before the manifests shipped) lack
//! `profile.yaml` / per-bridge `contract.yaml` in
//! their inlined tree. `migrate` walks the project's lineage, resolves
//! which units are inlined, and byte-copies the missing manifests from the
//! plugin tree's HEAD. Lineage `content_hash` updates to the new rolled-up
//! hash; `origin` / `origin_version` / `inlined_at` stay frozen.
//!
//! Coverage (this file ships 9 tests):
//!   1. Pre-manifest → manifest fixture migration: bootstrap with
//!      minimal_plugin, inline blocks/bridge with minimal_plugin, then
//!      point migrate at a synthetic manifest-shaped plugin tree
//!      (minimal_plugin + manifests). Profile.yaml and per-bridge
//!      contract.yaml land; lineage hashes advance;
//!      `origin` + `origin_version` + `inlined_at` stay byte-stable.
//!   2. Already-migrated no-op: bootstrap with full_plugin (manifests
//!      authored from the start), inline blocks/bridge from full_plugin,
//!      run migrate against full_plugin. Zero file writes; lineage
//!      byte-stable.
//!   3. Dry-run prints plan, doesn't touch disk.
//!   4. Pre-bootstrap project errors MIGRATE-001 cleanly.
//!   5. Plugin-tree missing block reference → MIGRATE-002 surfaces;
//!      remaining units still migrate; non-zero exit signal via the
//!      report's missing_in_plugin field.
//!   6. Local bridges skipped (no plugin source to byte-copy from).
//!   7. Audio-plugin templates copied byte-faithfully when missing.
//!   8. Re-running migrate is idempotent: second invocation is a true
//!      no-op (zero writes, lineage byte-stable).
//!   9. Migrate updates the upstream/ snapshot to match the new
//!      content_hash (so a follow-on `drift` reports clean).

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
    lineage::Lineage,
    migrate::{self, MigrateReport},
    upstream::{self, UnitKind},
};

// ---------------------------------------------------------------------------
// Fixture helpers

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
        "specdev-migrate-{tag}-{}-{}",
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

fn fresh_project(tag: &str, plugin: &Path, profile: &str) -> (PathBuf, LayoutContext) {
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

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Build a synthetic manifest-shaped plugin from the minimal_plugin tree
/// by copying minimal_plugin verbatim and overlaying the full_plugin
/// manifests onto the units that exist in BOTH trees (firmware-layered
/// profile + embassy block). For the embassy+lvgl bridge — which only
/// exists in minimal_plugin — we author a minimal contract.yaml inline so
/// the migrate target has something to copy.
fn build_synthetic_manifest_plugin(tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let plugin = workspace.join("_plugin");
    copy_dir_recursive(&minimal_plugin(), &plugin).unwrap();

    // Profile shell: copy full_plugin firmware-layered manifest and the
    // templates/ its paired_files name onto minimal_plugin.
    let profile_yaml_src = full_plugin().join("profiles/firmware-layered/profile.yaml");
    let profile_yaml_dst = plugin.join("profiles/firmware-layered/profile.yaml");
    std::fs::copy(&profile_yaml_src, &profile_yaml_dst).unwrap();
    copy_dir_recursive(
        &full_plugin().join("profiles/firmware-layered/templates"),
        &plugin.join("profiles/firmware-layered/templates"),
    )
    .unwrap();

    // Bridge: embassy+lvgl exists only in minimal_plugin — author a minimal
    // contract.yaml: bridge_name + diagnostic_template.
    let bridge_contract = plugin.join("bridges/embassy+lvgl/contract.yaml");
    std::fs::write(
        &bridge_contract,
        b"bridge_name: embassy+lvgl\nbinding_locality: endpoint_module\ndiagnostic_template: \"<{bridge_name}>: {parameter} unresolved\"\n",
    )
    .unwrap();

    plugin
}

// ===========================================================================
// 1 — Pre-manifest → manifest migration writes profile.yaml + per-block
//     per-bridge contract.yaml; lineage hashes advance; meta +
//     per-unit origin/inlined_at stay frozen.
// ===========================================================================

#[test]
fn pre_manifest_fixture_migrates_profile_and_bridge() {
    // Bootstrap + inline against minimal_plugin (no manifests anywhere).
    let (target, layout) = fresh_project(
        "pre-manifest-bootstrap",
        &minimal_plugin(),
        "firmware-layered",
    );
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &minimal_plugin(), "lvgl").unwrap();
    run_inline_bridge(&layout, &minimal_plugin(), "embassy+lvgl").unwrap();

    // Confirm pre-migrate state: no manifests on disk.
    assert!(!target.join("specs/specdev/profile.yaml").exists());
    assert!(!target
        .join("specs/specdev/reductions/bridges/embassy+lvgl.contract.yaml")
        .exists());

    let lineage_before = Lineage::load(&layout.specdev_root).unwrap();
    let embassy_origin_before = lineage_before.blocks["embassy"].origin.clone();
    let embassy_origin_version_before = lineage_before.blocks["embassy"].origin_version.clone();
    let embassy_inlined_at_before = lineage_before.blocks["embassy"].inlined_at.clone();
    let embassy_hash_before = lineage_before.blocks["embassy"].content_hash.clone();
    let bridge_hash_before = lineage_before.bridges["embassy+lvgl"].content_hash.clone();

    let synthetic = build_synthetic_manifest_plugin("pre-manifest-target");
    let report = migrate::run_migrate(&layout, &synthetic, false, false).unwrap();

    assert!(target.join("specs/specdev/profile.yaml").is_file());
    // Blocks contribute no manifest any more: nothing lands under codegen/.
    assert!(!target.join("specs/specdev/codegen").exists());
    assert!(target
        .join("specs/specdev/reductions/bridges/embassy+lvgl.contract.yaml")
        .is_file());

    assert!(
        !report.profile_files_written.is_empty(),
        "profile shell must report at least one file (profile.yaml)"
    );
    assert!(
        !report.migrated.iter().any(|m| m.name == "embassy"),
        "a block has no manifest to migrate"
    );
    assert!(
        report.migrated.iter().any(|m| m.name == "embassy+lvgl"),
        "bridge must appear in migrated set"
    );
    assert!(
        report.missing_in_plugin.is_empty(),
        "synthetic plugin has every lineage unit; missing list must be empty"
    );

    let lineage_after = Lineage::load(&layout.specdev_root).unwrap();
    let embassy_after = &lineage_after.blocks["embassy"];
    assert_eq!(
        embassy_after.content_hash, embassy_hash_before,
        "a block's file set is unchanged by migrate"
    );
    assert_ne!(
        lineage_after.bridges["embassy+lvgl"].content_hash, bridge_hash_before,
        "bridge content_hash must advance"
    );
    // Origin / origin_version / inlined_at frozen — record original inline.
    assert_eq!(embassy_after.origin, embassy_origin_before);
    assert_eq!(embassy_after.origin_version, embassy_origin_version_before);
    assert_eq!(embassy_after.inlined_at, embassy_inlined_at_before);
    // Meta is the bootstrap audit trail; never mutated by migrate.
    assert_eq!(lineage_after.meta, lineage_before.meta);
}

// ===========================================================================
// 2 — Already-migrated project: full_plugin bootstrap → migrate is a no-op.
// ===========================================================================

#[test]
fn already_migrated_project_is_noop() {
    let (_target, layout) = fresh_project(
        "post-manifest-bootstrap",
        &full_plugin(),
        "firmware-layered",
    );
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();

    let lineage_before = Lineage::load(&layout.specdev_root).unwrap();
    let report = migrate::run_migrate(&layout, &full_plugin(), false, false).unwrap();

    assert_eq!(
        report.total_files_written(),
        0,
        "no-op must write zero files"
    );
    assert!(report.migrated.is_empty(), "no migrated units expected");
    assert!(
        report.profile_files_written.is_empty(),
        "no profile-shell files expected (already in place)"
    );
    assert!(
        report.profile_skipped,
        "profile_skipped must be true when nothing was added to the shell"
    );
    let lineage_after = Lineage::load(&layout.specdev_root).unwrap();
    assert_eq!(
        lineage_before, lineage_after,
        "lineage must be byte-stable across no-op migrate"
    );
}

// ===========================================================================
// 3 — Dry-run prints the plan but never writes to disk.
// ===========================================================================

#[test]
fn dry_run_does_not_touch_disk() {
    let (target, layout) =
        fresh_project("dry-run-bootstrap", &minimal_plugin(), "firmware-layered");
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &minimal_plugin(), "lvgl").unwrap();
    run_inline_bridge(&layout, &minimal_plugin(), "embassy+lvgl").unwrap();

    let synthetic = build_synthetic_manifest_plugin("dry-run-target");
    let lineage_before_bytes = std::fs::read(layout.specdev_root.join("lineage.toml")).unwrap();

    // Pre-existing snapshot of every project file (relative paths +
    // bytes). Dry-run must leave this set byte-stable.
    let snapshot_before = snapshot_specdev_tree(&target);

    let report = migrate::run_migrate(&layout, &synthetic, true, false).unwrap();
    assert!(report.dry_run);
    assert!(!report.profile_files_written.is_empty());
    assert!(!report.migrated.is_empty());

    // No new files materialized.
    assert!(!target.join("specs/specdev/profile.yaml").exists());
    assert!(!target
        .join("specs/specdev/reductions/bridges/embassy+lvgl.contract.yaml")
        .exists());

    // Existing files unchanged.
    let lineage_after_bytes = std::fs::read(layout.specdev_root.join("lineage.toml")).unwrap();
    assert_eq!(
        lineage_before_bytes, lineage_after_bytes,
        "dry-run must not touch lineage.toml"
    );
    assert_eq!(
        snapshot_before,
        snapshot_specdev_tree(&target),
        "dry-run must not touch any specs/specdev/ file"
    );
}

// ===========================================================================
// 4 — Pre-bootstrap project errors MIGRATE-001 cleanly.
// ===========================================================================

#[test]
fn pre_bootstrap_project_errors_migrate_001() {
    let workspace = tmp_workspace("no-bootstrap");
    let target = workspace.join("proj");
    // Build the minimum-viable layout for `LayoutContext::discover` to
    // succeed: a spec.toml at the project root + an empty specs/specdev/
    // tree. Lineage::load returns Lineage::default() (meta = None), which
    // is the MIGRATE-001 trigger.
    std::fs::create_dir_all(target.join("specs/specdev/schemas/inlined/blocks")).unwrap();
    std::fs::create_dir_all(target.join("specs/specdev/schemas/inlined/bridges")).unwrap();
    std::fs::create_dir_all(target.join("specs/specdev/reductions/blocks")).unwrap();
    std::fs::create_dir_all(target.join("specs/specdev/reductions/bridges/local")).unwrap();
    std::fs::create_dir_all(target.join("specs/modules")).unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "1.0"

[project]
name = "no-bootstrap"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/specdev/domain.toml"),
        "name = \"firmware-layered\"\n",
    )
    .unwrap();
    // Project-level reduction file the layout walker needs.
    std::fs::write(target.join("specs/specdev/reductions/project.yaml"), "{}\n").unwrap();
    // Empty aggregator schema so layout discovery doesn't blow up.
    std::fs::write(
        target.join("specs/specdev/schemas/module.schema.yaml"),
        "$schema: https://json-schema.org/draft/2020-12/schema\n$id: dummy\ntype: object\n",
    )
    .unwrap();

    let layout = LayoutContext::discover(&target, None).unwrap();
    let err = migrate::run_migrate(&layout, &full_plugin(), false, false).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("MIGRATE-001"),
        "expected MIGRATE-001; got: {msg}"
    );
    assert!(
        msg.contains("pre-bootstrap"),
        "diagnostic must explain why; got: {msg}"
    );
}

// ===========================================================================
// 5 — MIGRATE-002 surfaces when a lineage block has no plugin source.
// ===========================================================================

#[test]
fn missing_plugin_unit_surfaces_migrate_002() {
    // Bootstrap + inline against minimal_plugin.
    let (_target, layout) = fresh_project("missing-block", &minimal_plugin(), "firmware-layered");
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &minimal_plugin(), "lvgl").unwrap();

    // Build a synthetic plugin tree that drops `lvgl` entirely.
    let synthetic = build_synthetic_manifest_plugin("missing-target");
    std::fs::remove_dir_all(synthetic.join("blocks/lvgl")).unwrap();

    let report = migrate::run_migrate(&layout, &synthetic, false, false).unwrap();

    assert!(
        report
            .missing_in_plugin
            .iter()
            .any(|m| m.contains("MIGRATE-002") && m.contains("lvgl")),
        "MIGRATE-002 must mention the missing block name; got: {:?}",
        report.missing_in_plugin
    );
    // Other units (profile shell + embassy block) still migrated.
    assert!(
        !report.profile_files_written.is_empty()
            || report.migrated.iter().any(|m| m.name == "embassy"),
        "remaining units must still migrate; got migrated={:?}, profile_files={:?}",
        report.migrated.iter().map(|m| &m.name).collect::<Vec<_>>(),
        report.profile_files_written
    );
    // Lineage entry for the missing unit is left untouched.
    let lineage_after = Lineage::load(&layout.specdev_root).unwrap();
    assert!(
        lineage_after.blocks.contains_key("lvgl"),
        "missing-in-plugin unit's lineage entry stays in place (don't lose audit trail)"
    );
}

// ===========================================================================
// 6 — Project-local bridges (no lineage entry) are not migrated.
// ===========================================================================

#[test]
fn local_bridges_skipped() {
    let (target, layout) = fresh_project("local-bridges", &minimal_plugin(), "firmware-layered");
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &minimal_plugin(), "lvgl").unwrap();
    run_inline_bridge(&layout, &minimal_plugin(), "embassy+lvgl").unwrap();

    // Author a project-local bridge (no lineage entry; lives under
    // reductions/bridges/local/).
    let local_bridge_dir = target.join("specs/specdev/reductions/bridges/local");
    std::fs::write(
        local_bridge_dir.join("custom-local.yaml"),
        "entities: []\nrelations: []\n",
    )
    .unwrap();

    let synthetic = build_synthetic_manifest_plugin("local-bridges-target");
    let report = migrate::run_migrate(&layout, &synthetic, false, false).unwrap();

    // Migrate touches the canonical bridge but not the local one — lineage
    // is the authoritative source of "what to migrate" and local bridges
    // have no entry there.
    let migrated_bridge_names: Vec<&str> = report
        .migrated
        .iter()
        .filter(|m| matches!(m.kind, UnitKind::Bridge))
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(migrated_bridge_names, vec!["embassy+lvgl"]);
    assert!(
        report
            .missing_in_plugin
            .iter()
            .all(|m| !m.contains("custom-local")),
        "local bridges must not surface in missing_in_plugin: {:?}",
        report.missing_in_plugin
    );

    // The local bridge file is untouched on disk.
    assert!(local_bridge_dir.join("custom-local.yaml").is_file());
}

// ===========================================================================
// 7 — Audio-plugin templates copied byte-faithfully when missing.
// ===========================================================================

#[test]
fn audio_plugin_templates_copied() {
    // Build a pre-manifest-style audio-plugin: bootstrap from a SYNTHETIC
    // plugin tree with the audio-plugin profile but NO `profile.yaml` and
    // NO `templates/` directory. After bootstrap, the project lacks both.
    // Then point migrate at the real full_plugin tree (which DOES carry
    // them) and assert the template file lands byte-faithfully.
    let synthetic_pre = tmp_workspace("audio-pre");
    let synthetic_pre_plugin = synthetic_pre.join("_plugin");
    copy_dir_recursive(&full_plugin(), &synthetic_pre_plugin).unwrap();
    // Strip manifests + templates to simulate a pre-manifest audio-plugin,
    // which named its language by a `codegen/<lang>/*.conventions.md` file.
    let pre = synthetic_pre_plugin.join("profiles/audio-plugin");
    let _ = std::fs::remove_file(pre.join("profile.yaml"));
    let _ = std::fs::remove_dir_all(pre.join("templates"));
    std::fs::create_dir_all(pre.join("codegen/rust")).unwrap();
    std::fs::write(pre.join("codegen/rust/module.conventions.md"), "# pre-manifest\n").unwrap();

    let (target, layout) = fresh_project("audio-bootstrap", &synthetic_pre_plugin, "audio-plugin");
    assert!(!target.join("specs/specdev/profile.yaml").exists());
    assert!(!target
        .join("specs/specdev/templates/specdev-ci.yml.template")
        .exists());

    // Migrate against the real full_plugin tree.
    let report = migrate::run_migrate(&layout, &full_plugin(), false, false).unwrap();
    assert!(report.missing_in_plugin.is_empty());

    let template_dst = target.join("specs/specdev/templates/specdev-ci.yml.template");
    assert!(template_dst.is_file(), "template must be copied");
    let template_src =
        full_plugin().join("profiles/audio-plugin/templates/specdev-ci.yml.template");
    assert_eq!(
        std::fs::read(&template_src).unwrap(),
        std::fs::read(&template_dst).unwrap(),
        "template body must be byte-faithful"
    );
}

// ===========================================================================
// 8 — Re-running migrate is idempotent: second invocation is a true no-op.
// ===========================================================================

#[test]
fn migrate_is_idempotent() {
    let (_target, layout) = fresh_project("idempotent", &minimal_plugin(), "firmware-layered");
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &minimal_plugin(), "lvgl").unwrap();
    run_inline_bridge(&layout, &minimal_plugin(), "embassy+lvgl").unwrap();

    let synthetic = build_synthetic_manifest_plugin("idempotent-target");

    let r1 = migrate::run_migrate(&layout, &synthetic, false, false).unwrap();
    assert!(r1.total_files_written() > 0, "first migrate must do work");
    assert!(!r1.migrated.is_empty());

    // Snapshot lineage + tree, then re-run.
    let lineage_after_first = Lineage::load(&layout.specdev_root).unwrap();
    let tree_after_first = snapshot_specdev_tree(_target_root(&layout));

    let r2 = migrate::run_migrate(&layout, &synthetic, false, false).unwrap();
    assert_eq!(
        r2.total_files_written(),
        0,
        "second migrate must write zero files"
    );
    assert!(
        r2.migrated.is_empty(),
        "second migrate must report no migrations"
    );

    let lineage_after_second = Lineage::load(&layout.specdev_root).unwrap();
    assert_eq!(
        lineage_after_first, lineage_after_second,
        "lineage must be byte-stable across idempotent re-run"
    );
    assert_eq!(
        tree_after_first,
        snapshot_specdev_tree(_target_root(&layout)),
        "specs/specdev/ tree must be byte-stable across idempotent re-run"
    );
}

// ===========================================================================
// 9 — Migrate updates the upstream/ snapshot to match new content_hash.
// ===========================================================================

#[test]
fn migrate_updates_upstream_snapshot() {
    let (_target, layout) = fresh_project("upstream", &minimal_plugin(), "firmware-layered");
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &minimal_plugin(), "lvgl").unwrap();
    run_inline_bridge(&layout, &minimal_plugin(), "embassy+lvgl").unwrap();

    // Pre-migrate upstream snapshot has no contract.yaml.
    let upstream_dir = upstream::upstream_dir(&layout.specdev_root, UnitKind::Bridge, "embassy+lvgl");
    assert!(upstream_dir.is_dir());
    assert!(!upstream_dir.join("contract.yaml").exists());

    let synthetic = build_synthetic_manifest_plugin("upstream-target");
    let report: MigrateReport = migrate::run_migrate(&layout, &synthetic, false, false).unwrap();
    assert!(report.migrated.iter().any(|m| m.name == "embassy+lvgl"));

    // Post-migrate upstream snapshot includes contract.yaml.
    let upstream_contract = upstream_dir.join("contract.yaml");
    assert!(
        upstream_contract.is_file(),
        "upstream snapshot must reflect post-migrate file set"
    );
    let plugin_contract = synthetic.join("bridges/embassy+lvgl/contract.yaml");
    assert_eq!(
        std::fs::read(&plugin_contract).unwrap(),
        std::fs::read(&upstream_contract).unwrap(),
        "upstream bytes must equal plugin-HEAD bytes"
    );
}

// ---------------------------------------------------------------------------
// Helpers

fn _target_root(layout: &LayoutContext) -> &Path {
    layout
        .specdev_root
        .parent()
        .and_then(|p| p.parent())
        .expect("specdev_root must be at <project>/specs/specdev")
}

/// Snapshot every file under `<target>/specs/specdev/` as a sorted vec
/// of `(rel_path, bytes)`. Used to assert byte-stability across dry-run
/// and idempotent re-runs.
fn snapshot_specdev_tree(target: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let root = target.join("specs/specdev");
    let mut out: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    walk_collect(&root, &root, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn walk_collect(base: &Path, cur: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(cur) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_collect(base, &path, out);
        } else if path.is_file() {
            let rel = path.strip_prefix(base).unwrap().to_path_buf();
            let bytes = std::fs::read(&path).unwrap();
            out.push((rel, bytes));
        }
    }
}
