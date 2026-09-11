//! `--merge` and `drift`.
//!
//! Behaviors under test:
//!
//! 1. Local edit + upstream compatible change → `--merge` clean, lineage advances.
//! 2. Local edit + upstream conflicting change → conflict markers, lineage unchanged.
//! 3. `drift` snapshot-testable across all four categories.
//! 4. Lineage-always-written invariant — drift catches missing entries.
//! 5. Deterministic-inline invariant — clean re-merge zero diff.
//! 6. Semantically-broken-merge case — clean merge succeeds, validate catches breakage.
//! 7. `--merge` on never-inlined unit errors `MERGE-003`.
//!
//! These tests reuse the minimal plugin fixture as the "stationary" plugin and
//! construct "moved" variants per-scenario at test time via
//! [`clone_plugin_with_modifications`]. This avoids fixture sprawl while
//! keeping scenarios self-contained.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    drift,
    inline::run_inline_block,
    layout::LayoutContext,
    lineage::Lineage,
    merge,
};

fn minimal_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("minimal_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-merge-drift-{tag}-{}-{}",
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

/// Recursively copy a directory tree.
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let s = entry.path();
        let d = dst.join(entry.file_name());
        if s.is_dir() {
            copy_tree(&s, &d);
        } else {
            std::fs::copy(&s, &d).unwrap();
        }
    }
}

/// Clone the minimal plugin fixture into a fresh temp dir and overwrite
/// the listed paths (relative to the plugin root) with the given bytes.
/// Returns the cloned plugin's root.
fn clone_plugin_with_modifications(
    workspace: &Path,
    modifications: BTreeMap<&str, &[u8]>,
) -> PathBuf {
    let dst = workspace.join("plugin_moved");
    copy_tree(&minimal_plugin(), &dst);
    for (rel, bytes) in modifications {
        let target = dst.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(target, bytes).unwrap();
    }
    dst
}

/// Bootstrap a fresh project + inline embassy. Returns `(workspace, project_root, layout)`.
fn fresh_project_with_embassy(tag: &str) -> (PathBuf, PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let project = workspace.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    bootstrap::run_bootstrap(
        &minimal_plugin(),
        &BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: project.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&project, None).unwrap();
    run_inline_block(&layout, &minimal_plugin(), "embassy").unwrap();
    (workspace, project, layout)
}

const EMBASSY_SCHEMA_BASE: &str =
    include_str!("specs/minimal_plugin/_plugin/blocks/embassy/schema.yaml");

// ---------------------------------------------------------------------------
// Exit criterion 5 — clean re-merge (theirs == base) is zero-diff, content_hash unchanged.
// ---------------------------------------------------------------------------
#[test]
fn merge_clean_reinline_is_zero_diff() {
    let (_ws, _proj, layout) = fresh_project_with_embassy("clean-rerun");
    let lineage_before = Lineage::load(&layout.specdev_root).unwrap();
    let hash_before = lineage_before.blocks["embassy"].content_hash.clone();

    let bytes_before = std::fs::read(
        layout
            .specdev_root
            .join("schemas/inlined/blocks/embassy.schema.yaml"),
    )
    .unwrap();

    // Plugin unchanged → merge against itself.
    let outcome =
        merge::run_merge_block(&layout, &minimal_plugin(), "embassy").expect("clean re-merge");
    assert!(outcome.clean);

    let bytes_after = std::fs::read(
        layout
            .specdev_root
            .join("schemas/inlined/blocks/embassy.schema.yaml"),
    )
    .unwrap();
    assert_eq!(
        bytes_before, bytes_after,
        "files must not drift on no-op merge"
    );

    let lineage_after = Lineage::load(&layout.specdev_root).unwrap();
    assert_eq!(
        lineage_after.blocks["embassy"].content_hash, hash_before,
        "content_hash must be stable on no-op merge"
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 1 — local edit + plugin edit on different lines = clean 3-way.
// ---------------------------------------------------------------------------
#[test]
fn merge_compatible_edits_lands_both() {
    let (ws, _proj, layout) = fresh_project_with_embassy("compatible");

    // Local edit: change `priority` description in schema.
    let project_schema_path = layout
        .specdev_root
        .join("schemas/inlined/blocks/embassy.schema.yaml");
    let local = String::from(EMBASSY_SCHEMA_BASE).replace(
        "priority: { type: string }",
        "priority: { type: string, description: \"local edit added this\" }",
    );
    std::fs::write(&project_schema_path, &local).unwrap();

    // Plugin edit: bump $id version.
    let mut mods = BTreeMap::new();
    let plugin_modified = String::from(EMBASSY_SCHEMA_BASE).replace(
        "https://specdev.dev/blocks/embassy.v1",
        "https://specdev.dev/blocks/embassy.v1-rev2",
    );
    let plugin_bytes = plugin_modified.as_bytes();
    mods.insert("blocks/embassy/schema.yaml", plugin_bytes);
    // Bump plugin version too.
    let bumped_manifest = b"{\"name\":\"specdev-fixture\",\"version\":\"0.0.2-fixture\"}\n";
    mods.insert(".claude-plugin/plugin.json", bumped_manifest.as_slice());
    let moved_plugin = clone_plugin_with_modifications(&ws, mods);

    let outcome =
        merge::run_merge_block(&layout, &moved_plugin, "embassy").expect("clean 3-way merge");
    assert!(outcome.clean);
    assert_eq!(
        outcome.advanced_to_version.as_deref(),
        Some("0.0.2-fixture"),
        "lineage version should advance to the new plugin version"
    );

    let merged = std::fs::read_to_string(&project_schema_path).unwrap();
    assert!(
        merged.contains("local edit added this"),
        "local edit must survive: {merged}"
    );
    assert!(
        merged.contains("embassy.v1-rev2"),
        "upstream edit must apply: {merged}"
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 2 — local + plugin edit same line → conflict markers, lineage unchanged.
// ---------------------------------------------------------------------------
#[test]
fn merge_conflicting_edits_leaves_markers_and_preserves_lineage() {
    let (ws, _proj, layout) = fresh_project_with_embassy("conflict");

    // Use a synthetic edit that's hard for line-based merge to reconcile:
    // both sides rewrite the same line with mutually-exclusive content.
    // The base schema's `type: object` line is short and unique; rewriting
    // it with two different content forces a conflict.
    let project_schema_path = layout
        .specdev_root
        .join("schemas/inlined/blocks/embassy.schema.yaml");
    let local = String::from(EMBASSY_SCHEMA_BASE).replace(
        "type: object",
        "type: object\n# local-only annotation\nfoo: bar",
    );
    std::fs::write(&project_schema_path, &local).unwrap();

    let mut mods = BTreeMap::new();
    let plugin_modified = String::from(EMBASSY_SCHEMA_BASE).replace(
        "type: object",
        "type: object\n# upstream-only annotation\nbaz: qux",
    );
    mods.insert("blocks/embassy/schema.yaml", plugin_modified.as_bytes());
    let moved_plugin = clone_plugin_with_modifications(&ws, mods);

    let lineage_before = Lineage::load(&layout.specdev_root).unwrap();
    let hash_before = lineage_before.blocks["embassy"].content_hash.clone();

    let err = merge::run_merge_block(&layout, &moved_plugin, "embassy").unwrap_err();
    let s = format!("{err:#}");
    assert!(s.contains("MERGE-002"), "{s}");

    let after = std::fs::read_to_string(&project_schema_path).unwrap();
    assert!(
        after.contains("<<<<<<<"),
        "conflict markers expected: {after}"
    );
    assert!(
        after.contains(">>>>>>>"),
        "conflict markers expected: {after}"
    );

    let lineage_after = Lineage::load(&layout.specdev_root).unwrap();
    assert_eq!(
        lineage_after.blocks["embassy"].content_hash, hash_before,
        "lineage must be unchanged on conflict"
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 7 — `--merge` on never-inlined unit errors MERGE-003.
// ---------------------------------------------------------------------------
#[test]
fn merge_on_never_inlined_errors_merge_003() {
    let workspace = tmp_workspace("merge-003");
    let project = workspace.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    bootstrap::run_bootstrap(
        &minimal_plugin(),
        &BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: project.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&project, None).unwrap();

    let err = merge::run_merge_block(&layout, &minimal_plugin(), "embassy").unwrap_err();
    let s = format!("{err:#}");
    assert!(s.contains("MERGE-003"), "{s}");
}

// ---------------------------------------------------------------------------
// Exit criterion 3a — drift "Clean" category when nothing has changed.
// ---------------------------------------------------------------------------
#[test]
fn drift_reports_clean_when_no_changes() {
    let (_ws, _proj, layout) = fresh_project_with_embassy("drift-clean");
    let report = drift::run_drift(&layout, &minimal_plugin()).unwrap();
    assert!(
        report.is_clean(),
        "expected clean drift; got:\n{}",
        report.to_yaml().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 3b — "PluginMoved" category when only plugin changed.
// ---------------------------------------------------------------------------
#[test]
fn drift_reports_plugin_moved() {
    let (ws, _proj, layout) = fresh_project_with_embassy("drift-plugin-moved");
    let mut mods = BTreeMap::new();
    let plugin_modified = String::from(EMBASSY_SCHEMA_BASE).replace(
        "https://specdev.dev/blocks/embassy.v1",
        "https://specdev.dev/blocks/embassy.v1-moved",
    );
    mods.insert("blocks/embassy/schema.yaml", plugin_modified.as_bytes());
    let moved_plugin = clone_plugin_with_modifications(&ws, mods);

    let report = drift::run_drift(&layout, &moved_plugin).unwrap();
    let categories: Vec<String> = report
        .units
        .iter()
        .flat_map(|u| u.files.iter().map(|f| format!("{:?}", f.category)))
        .collect();
    assert!(
        categories.contains(&"PluginMoved".to_string()),
        "expected PluginMoved category; got: {categories:?}\n{}",
        report.to_yaml().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 3c — "LocallyEdited" category when only project changed.
// ---------------------------------------------------------------------------
#[test]
fn drift_reports_locally_edited() {
    let (_ws, _proj, layout) = fresh_project_with_embassy("drift-local");
    let project_schema = layout
        .specdev_root
        .join("schemas/inlined/blocks/embassy.schema.yaml");
    std::fs::write(
        &project_schema,
        format!("{}\n# locally added comment\n", EMBASSY_SCHEMA_BASE),
    )
    .unwrap();

    let report = drift::run_drift(&layout, &minimal_plugin()).unwrap();
    let categories: Vec<String> = report
        .units
        .iter()
        .flat_map(|u| u.files.iter().map(|f| format!("{:?}", f.category)))
        .collect();
    assert!(
        categories.contains(&"LocallyEdited".to_string()),
        "expected LocallyEdited; got: {categories:?}\n{}",
        report.to_yaml().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 3d — "Both" category when project and plugin both changed.
// ---------------------------------------------------------------------------
#[test]
fn drift_reports_both() {
    let (ws, _proj, layout) = fresh_project_with_embassy("drift-both");
    // Local edit
    let project_schema = layout
        .specdev_root
        .join("schemas/inlined/blocks/embassy.schema.yaml");
    std::fs::write(
        &project_schema,
        format!("{}\n# locally added\n", EMBASSY_SCHEMA_BASE),
    )
    .unwrap();
    // Plugin edit
    let mut mods = BTreeMap::new();
    mods.insert(
        "blocks/embassy/schema.yaml",
        format!("{}\n# plugin added\n", EMBASSY_SCHEMA_BASE).into_bytes(),
    );
    // Boxed Vec<u8> would be cleaner, but BTreeMap<&str, &[u8]> wants
    // borrowed slices; use a stable owner.
    let plugin_bytes: Vec<u8> = mods.get("blocks/embassy/schema.yaml").unwrap().to_vec();
    let mut mods_borrow: BTreeMap<&str, &[u8]> = BTreeMap::new();
    mods_borrow.insert("blocks/embassy/schema.yaml", plugin_bytes.as_slice());
    let moved_plugin = clone_plugin_with_modifications(&ws, mods_borrow);

    let report = drift::run_drift(&layout, &moved_plugin).unwrap();
    let categories: Vec<String> = report
        .units
        .iter()
        .flat_map(|u| u.files.iter().map(|f| format!("{:?}", f.category)))
        .collect();
    assert!(
        categories.contains(&"Both".to_string()),
        "expected Both; got: {categories:?}\n{}",
        report.to_yaml().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 4 — DRIFT-INVARIANT-001 when an inlined file has no upstream entry.
// ---------------------------------------------------------------------------
#[test]
fn drift_reports_invariant_001_when_upstream_file_deleted() {
    let (_ws, _proj, layout) = fresh_project_with_embassy("drift-inv-001");
    // Hand-delete one upstream file but leave the project copy.
    std::fs::remove_file(
        layout
            .specdev_root
            .join("upstream/blocks/embassy/schema.yaml"),
    )
    .unwrap();

    let report = drift::run_drift(&layout, &minimal_plugin()).unwrap();
    assert!(
        report
            .invariants
            .iter()
            .any(|i| i.code == "DRIFT-INVARIANT-001"),
        "expected DRIFT-INVARIANT-001; got:\n{}",
        report.to_yaml().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Exit criterion 4 (cont.) — DRIFT-INVARIANT-002 when upstream entry has no inlined file.
// ---------------------------------------------------------------------------
#[test]
fn drift_reports_invariant_002_when_inlined_file_deleted() {
    let (_ws, _proj, layout) = fresh_project_with_embassy("drift-inv-002");
    std::fs::remove_file(
        layout
            .specdev_root
            .join("schemas/inlined/blocks/embassy.schema.yaml"),
    )
    .unwrap();

    let report = drift::run_drift(&layout, &minimal_plugin()).unwrap();
    assert!(
        report
            .invariants
            .iter()
            .any(|i| i.code == "DRIFT-INVARIANT-002"),
        "expected DRIFT-INVARIANT-002; got:\n{}",
        report.to_yaml().unwrap()
    );
}

// ---------------------------------------------------------------------------
// Snapshot — drift output for the clean baseline (a stable witness across
// runs that catches structural changes to the report shape).
// ---------------------------------------------------------------------------
#[test]
fn snapshot_drift_clean() {
    let (_ws, _proj, layout) = fresh_project_with_embassy("drift-snapshot");
    let report = drift::run_drift(&layout, &minimal_plugin()).unwrap();
    insta::assert_snapshot!(report.to_yaml().unwrap());
}
