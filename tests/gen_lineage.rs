//! Integration tests for the gen-lineage tripwire: `gen-record` machinery
//! (`gen_lineage::record_files`) + the `report gen-drift` classifier
//! (`gen_lineage::run_gen_drift`) + the `report gen-plan` incremental
//! classifier (`gen_lineage::run_gen_plan`).

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::gen_lineage::{
    gen_lineage_path, record_files, record_files_accepting_edits, run_gen_drift, run_gen_plan,
    GenFileCategory, GenLineage, GenPlanCategory,
};
use specdev_analyzer::layout::LayoutContext;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir()
        .join("specdev-gen-lineage-it")
        .join(format!("{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// Minimal project: spec.toml + specs/specdev/ + src/, with an inlined
/// profile that declares one module-scope gate (`cargo-check`) so
/// `verified` has a registry to be computed against.
fn minimal_project(root: &Path) -> LayoutContext {
    let layout = minimal_project_without_registry(root);
    write_profile(
        root,
        "gates:\n  - label: cargo-check\n    scope: module\n    steps: [[cargo, check, --quiet]]\n",
    );
    layout
}

/// The inlined `profile.yaml` for the test project, with `extra` appended
/// after the required keys (a `gates:` block or nothing).
fn write_profile(root: &Path, extra: &str) {
    fs::write(
        root.join("specs/specdev/profile.yaml"),
        format!("name: test-profile\nlanguage: rust\n{extra}"),
    )
    .unwrap();
}

/// Same project with no inlined `profile.yaml` at all — a pre-manifest
/// tree, or one bootstrapped before the registry key existed.
fn minimal_project_without_registry(root: &Path) -> LayoutContext {
    fs::create_dir_all(root.join("specs/specdev")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("spec.toml"),
        r#"
spec_version = "1.0"

[project]
name = "genlineage-fixture"
profile = "test-profile"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    LayoutContext::discover(root, None).expect("discover")
}

const TS: &str = "2026-06-12T10:00:00Z";

/// Author a bare module spec so `record_files` can assemble its prompt and
/// compute an `input_hash`. A name + layer is enough — the prompt assembler
/// fills missing conventions/architecture with `(no ...)` notes.
fn write_module(root: &Path, name: &str, body: &str) {
    fs::create_dir_all(root.join("specs/modules")).unwrap();
    fs::write(
        root.join(format!("specs/modules/{name}.toml")),
        format!("name = \"{name}\"\nlayer = \"driver\"\n{body}"),
    )
    .unwrap();
}

#[test]
fn records_input_hash_for_a_real_module() {
    let root = tmp("input-hash");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();

    record_files(&layout, Some("Widget"), &[src], TS).unwrap();
    let lineage = GenLineage::load(&layout.specdev_root).unwrap();
    let entry = lineage.files.get("src/widget.rs").expect("entry");
    let hash = entry.input_hash.as_deref().expect("input_hash recorded");
    assert!(hash.starts_with("sha256:"), "{hash}");
}

#[test]
fn input_hash_is_stable_and_moves_with_the_spec() {
    let root = tmp("input-hash-sensitivity");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();

    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();
    let h1 = GenLineage::load(&layout.specdev_root)
        .unwrap()
        .files
        .get("src/widget.rs")
        .unwrap()
        .input_hash
        .clone()
        .unwrap();

    // Re-recording an unchanged spec yields the same input_hash.
    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();
    let h2 = GenLineage::load(&layout.specdev_root)
        .unwrap()
        .files
        .get("src/widget.rs")
        .unwrap()
        .input_hash
        .clone()
        .unwrap();
    assert_eq!(h1, h2, "unchanged spec must keep the same input_hash");

    // Editing the module spec changes the assembled prompt → the hash moves.
    write_module(&root, "Widget", "description = \"now documented\"\n");
    record_files(&layout, Some("Widget"), &[src], TS).unwrap();
    let h3 = GenLineage::load(&layout.specdev_root)
        .unwrap()
        .files
        .get("src/widget.rs")
        .unwrap()
        .input_hash
        .clone()
        .unwrap();
    assert_ne!(h1, h3, "an edited spec must move the input_hash");
}

#[test]
fn project_level_file_has_no_input_hash() {
    let root = tmp("input-hash-none");
    let layout = minimal_project(&root);
    let cargo = root.join("Cargo.toml");
    fs::write(&cargo, "[package]\nname = \"x\"\n").unwrap();

    record_files(&layout, None, &[cargo], TS).unwrap();
    let entry = GenLineage::load(&layout.specdev_root)
        .unwrap()
        .files
        .get("Cargo.toml")
        .cloned()
        .unwrap();
    assert_eq!(entry.input_hash, None);
}

#[test]
fn missing_module_spec_records_output_but_no_input_hash() {
    // Fail-safe: a module whose spec can't be assembled still records its
    // output hash, but with input_hash = None so it never skips on a hash it
    // could not compute. (Widget has no specs/modules/Widget.toml here.)
    let root = tmp("input-hash-failsafe");
    let layout = minimal_project(&root);
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();

    record_files(&layout, Some("Widget"), &[src], TS).unwrap();
    let entry = GenLineage::load(&layout.specdev_root)
        .unwrap()
        .files
        .get("src/widget.rs")
        .cloned()
        .unwrap();
    assert_eq!(entry.module.as_deref(), Some("Widget"));
    assert!(entry.content_hash.starts_with("sha256:"));
    assert_eq!(
        entry.input_hash, None,
        "unassemblable module → no input_hash"
    );
}

#[test]
fn record_then_report_is_clean() {
    let root = tmp("clean");
    let layout = minimal_project(&root);
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();

    let recorded = record_files(&layout, Some("Widget"), &[src], TS).unwrap();
    assert_eq!(recorded, vec!["src/widget.rs".to_string()]);
    assert!(gen_lineage_path(&layout.specdev_root).is_file());

    let report = run_gen_drift(&layout).unwrap();
    assert!(report.is_clean());
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].path, "src/widget.rs");
    assert_eq!(report.files[0].category, GenFileCategory::Clean);
    assert_eq!(report.files[0].module.as_deref(), Some("Widget"));
}

#[test]
fn hand_edit_is_flagged_edited() {
    let root = tmp("edited");
    let layout = minimal_project(&root);
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();

    fs::write(&src, "pub fn widget() { /* hand edit */ }\n").unwrap();

    let report = run_gen_drift(&layout).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.files[0].category, GenFileCategory::Edited);
}

#[test]
fn deleted_file_is_flagged_missing() {
    let root = tmp("missing");
    let layout = minimal_project(&root);
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();

    fs::remove_file(&src).unwrap();

    let report = run_gen_drift(&layout).unwrap();
    assert!(!report.is_clean());
    assert_eq!(report.files[0].category, GenFileCategory::Missing);
}

/// Regeneration legitimately moves the baseline: `/spec-gen` re-emits a file,
/// its bytes differ from the last recording, and recording the new bytes is
/// correct. That path must declare itself, because the alternative — a bare
/// re-record — is how a hand-edit gets laundered into `clean`
/// (`gen_record_edit_guard.rs`). Byte-difference alone cannot tell a fresh
/// emission from a hand-edit, so the caller states which it is.
#[test]
fn re_record_returns_edited_file_to_clean() {
    let root = tmp("rerecord");
    let layout = minimal_project(&root);
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();

    fs::write(&src, "pub fn widget() { /* regenerated */ }\n").unwrap();
    assert!(!run_gen_drift(&layout).unwrap().is_clean());

    record_files_accepting_edits(&layout, Some("Widget"), &[src], "2026-06-12T11:00:00Z").unwrap();
    let report = run_gen_drift(&layout).unwrap();
    assert!(report.is_clean());
    // Upsert replaced, not appended.
    assert_eq!(report.files.len(), 1);
    assert_eq!(report.files[0].generated_at, "2026-06-12T11:00:00Z");
}

#[test]
fn project_level_file_records_without_module() {
    let root = tmp("nomodule");
    let layout = minimal_project(&root);
    let cargo = root.join("Cargo.toml");
    fs::write(&cargo, "[package]\nname = \"x\"\n").unwrap();

    record_files(&layout, None, &[cargo], TS).unwrap();
    let report = run_gen_drift(&layout).unwrap();
    assert_eq!(report.files[0].path, "Cargo.toml");
    assert_eq!(report.files[0].module, None);
}

#[test]
fn file_outside_project_root_is_rejected() {
    let root = tmp("outside");
    let layout = minimal_project(&root);
    let stray = tmp("outside-stray").join("stray.rs");
    fs::write(&stray, "x").unwrap();

    let err = record_files(&layout, None, &[stray], TS).unwrap_err();
    assert!(
        format!("{err:#}").contains("outside the project root"),
        "{err:#}"
    );
}

#[test]
fn nonexistent_file_is_rejected() {
    let root = tmp("nonexistent");
    let layout = minimal_project(&root);
    let err = record_files(&layout, None, &[root.join("src/ghost.rs")], TS).unwrap_err();
    assert!(format!("{err:#}").contains("does not exist"), "{err:#}");
}

#[test]
fn empty_file_list_is_rejected() {
    let root = tmp("emptylist");
    let layout = minimal_project(&root);
    let err = record_files(&layout, None, &[], TS).unwrap_err();
    assert!(format!("{err:#}").contains("no files given"), "{err:#}");
}

#[test]
fn report_is_sorted_by_path_and_yaml_stable() {
    let root = tmp("sorted");
    let layout = minimal_project(&root);
    let b = root.join("src/b.rs");
    let a = root.join("src/a.rs");
    fs::write(&b, "b").unwrap();
    fs::write(&a, "a").unwrap();
    // Record in reverse-lex order; BTreeMap restores lex order.
    record_files(&layout, Some("M"), &[b, a], TS).unwrap();

    let report = run_gen_drift(&layout).unwrap();
    let paths: Vec<&str> = report.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "src/b.rs"]);

    let yaml1 = report.to_yaml().unwrap();
    let yaml2 = run_gen_drift(&layout).unwrap().to_yaml().unwrap();
    assert_eq!(yaml1, yaml2);
}

#[test]
fn no_record_file_reports_empty_and_clean() {
    let root = tmp("norecord");
    let layout = minimal_project(&root);
    let report = run_gen_drift(&layout).unwrap();
    assert!(report.files.is_empty());
    assert!(report.is_clean());
}

#[test]
fn malformed_record_file_surfaces_gen_lineage_001() {
    let root = tmp("malformed-it");
    let layout = minimal_project(&root);
    fs::write(gen_lineage_path(&layout.specdev_root), "not = valid = toml").unwrap();
    let err = run_gen_drift(&layout).unwrap_err();
    assert!(format!("{err:#}").contains("GEN-LINEAGE-001"), "{err:#}");
}

#[test]
fn record_file_round_trips_through_load() {
    let root = tmp("roundtrip-it");
    let layout = minimal_project(&root);
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src], TS).unwrap();

    let lineage = GenLineage::load(&layout.specdev_root).unwrap();
    let entry = lineage.files.get("src/widget.rs").expect("entry");
    assert_eq!(entry.module.as_deref(), Some("Widget"));
    assert_eq!(entry.generated_at, TS);
    assert!(entry.content_hash.starts_with("sha256:"));
}

// ---------------------------------------------------------------------------
// gen-plan: per-module incremental classification.
// ---------------------------------------------------------------------------

/// Look up one module's verdict in the plan.
fn category_of(layout: &LayoutContext, module: &str) -> GenPlanCategory {
    run_gen_plan(layout)
        .unwrap()
        .modules
        .into_iter()
        .find(|m| m.module == module)
        .unwrap_or_else(|| panic!("module `{module}` not in plan"))
        .category
}

#[test]
fn plan_module_with_no_lineage_is_new() {
    let root = tmp("plan-new");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    // Spec authored, never generated → new.
    assert_eq!(category_of(&layout, "Widget"), GenPlanCategory::New);
}

#[test]
fn plan_clean_output_matching_input_is_unchanged() {
    let root = tmp("plan-unchanged");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src], TS).unwrap();

    // Output untouched + spec untouched → skippable.
    assert_eq!(category_of(&layout, "Widget"), GenPlanCategory::Unchanged);
}

#[test]
fn plan_clean_output_changed_input_is_stale() {
    let root = tmp("plan-stale");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src], TS).unwrap();

    // Edit the spec (the input) without regenerating → inputs moved.
    write_module(&root, "Widget", "description = \"now documented\"\n");
    assert_eq!(category_of(&layout, "Widget"), GenPlanCategory::Stale);
}

#[test]
fn plan_missing_recorded_input_hash_is_stale() {
    // A lineage recorded before input_hash existed (or via the fail-safe
    // path) has a clean output but no input_hash → cannot prove unchanged →
    // stale, never wrongly skipped.
    let root = tmp("plan-noinput");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    // Record with module=None semantics for input by recording before the
    // spec exists is awkward; instead hand-build a lineage entry with no
    // input_hash by recording, then stripping it.
    record_files(&layout, Some("Widget"), &[src], TS).unwrap();
    let mut lineage = GenLineage::load(&layout.specdev_root).unwrap();
    lineage.files.get_mut("src/widget.rs").unwrap().input_hash = None;
    lineage.save(&layout.specdev_root).unwrap();

    assert_eq!(category_of(&layout, "Widget"), GenPlanCategory::Stale);
}

#[test]
fn plan_hand_edited_output_is_edited() {
    let root = tmp("plan-edited");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();

    fs::write(&src, "pub fn widget() { /* hand edit */ }\n").unwrap();
    let plan = run_gen_plan(&layout).unwrap();
    let m = plan.modules.iter().find(|m| m.module == "Widget").unwrap();
    assert_eq!(m.category, GenPlanCategory::Edited);
    assert_eq!(m.files, vec!["src/widget.rs".to_string()]);
    assert!(!m.category.regenerates(), "edited modules are skipped");
}

#[test]
fn plan_deleted_output_is_missing() {
    let root = tmp("plan-missing");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();

    fs::remove_file(&src).unwrap();
    let plan = run_gen_plan(&layout).unwrap();
    let m = plan.modules.iter().find(|m| m.module == "Widget").unwrap();
    assert_eq!(m.category, GenPlanCategory::Missing);
    assert_eq!(m.files, vec!["src/widget.rs".to_string()]);
    assert!(m.category.regenerates());
}

#[test]
fn plan_edited_outranks_missing() {
    // A module whose outputs include both a hand-edited file and a deleted
    // file classifies as `edited` (no-clobber wins): regenerating would
    // overwrite the hand-edit.
    let root = tmp("plan-precedence");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let a = root.join("src/a.rs");
    let b = root.join("src/b.rs");
    fs::write(&a, "pub fn a() {}\n").unwrap();
    fs::write(&b, "pub fn b() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[a.clone(), b.clone()], TS).unwrap();

    fs::write(&a, "pub fn a() { /* edit */ }\n").unwrap(); // edited
    fs::remove_file(&b).unwrap(); // missing
    assert_eq!(category_of(&layout, "Widget"), GenPlanCategory::Edited);
}

#[test]
fn plan_covers_every_module_and_is_yaml_stable() {
    let root = tmp("plan-multi");
    let layout = minimal_project(&root);
    write_module(&root, "Alpha", "");
    write_module(&root, "Beta", "");
    let alpha = root.join("src/alpha.rs");
    fs::write(&alpha, "pub fn alpha() {}\n").unwrap();
    record_files(&layout, Some("Alpha"), &[alpha], TS).unwrap();
    // Beta has a spec but was never generated.

    let plan = run_gen_plan(&layout).unwrap();
    let names: Vec<&str> = plan.modules.iter().map(|m| m.module.as_str()).collect();
    assert_eq!(names, ["Alpha", "Beta"], "sorted, one verdict per module");
    assert_eq!(plan.modules[0].category, GenPlanCategory::Unchanged);
    assert_eq!(plan.modules[1].category, GenPlanCategory::New);

    let y1 = plan.to_yaml().unwrap();
    let y2 = run_gen_plan(&layout).unwrap().to_yaml().unwrap();
    assert_eq!(y1, y2);
}

// ---------------------------------------------------------------------------
// Compile-gate results: the `gates` table + gen-plan's `verified` axis.
// ---------------------------------------------------------------------------

use specdev_analyzer::gen_lineage::{
    compose_gate_key, lockfile_hash, modules_hash, profile_hash, record_gate, GateKeyBasis,
    KEY_NONE,
};

/// Plan row for one module (category + verified).
fn plan_row(layout: &LayoutContext, module: &str) -> (GenPlanCategory, bool) {
    let m = run_gen_plan(layout)
        .unwrap()
        .modules
        .into_iter()
        .find(|m| m.module == module)
        .unwrap_or_else(|| panic!("module `{module}` not in plan"));
    (m.category, m.verified)
}

/// A generated + recorded module whose files are clean and inputs match:
/// `unchanged` before any gate is recorded.
fn unchanged_module(root: &Path, layout: &LayoutContext, name: &str) {
    write_module(root, name, "");
    let src = root.join(format!("src/{}.rs", name.to_lowercase()));
    fs::write(&src, format!("pub fn {}() {{}}\n", name.to_lowercase())).unwrap();
    record_files(layout, Some(name), &[src], TS).unwrap();
    assert_eq!(category_of(layout, name), GenPlanCategory::Unchanged);
}

#[test]
fn gate_fail_row_leaves_module_unchanged_but_unverified() {
    // #107: the skip hole. Before this table a failed gate left the module
    // `unchanged` with nothing to say it never compiled.
    let root = tmp("gate-fail");
    let layout = minimal_project(&root);
    unchanged_module(&root, &layout, "Widget");
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );

    record_gate(&layout, &["Widget".to_string()], "cargo-check", false).unwrap();
    // Still `unchanged` on the regenerate axis — nothing about its inputs or
    // outputs moved — but `verified == false`, which is the re-gate set.
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );

    record_gate(&layout, &["Widget".to_string()], "cargo-check", true).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, true)
    );
}

#[test]
fn gate_pass_is_invalidated_by_a_lockfile_change_without_regenerating() {
    let root = tmp("gate-lock");
    let layout = minimal_project(&root);
    unchanged_module(&root, &layout, "Widget");
    fs::write(root.join("Cargo.lock"), "# lock v1\n").unwrap();
    record_gate(&layout, &["Widget".to_string()], "cargo-check", true).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, true)
    );

    // A `cargo update`: lockfile moves, spec and source do not.
    fs::write(root.join("Cargo.lock"), "# lock v2\n").unwrap();
    // Regenerate axis untouched; verification axis dropped. The skill
    // answers this with `cargo check`, not with a prompt.
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );
}

#[test]
fn gate_pass_is_invalidated_by_a_spec_edit() {
    let root = tmp("gate-spec");
    let layout = minimal_project(&root);
    unchanged_module(&root, &layout, "Widget");
    record_gate(&layout, &["Widget".to_string()], "cargo-check", true).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, true)
    );

    write_module(&root, "Widget", "description = \"moved\"\n");
    // `stale` regenerates anyway; `verified` must agree (the pass was about
    // the old inputs).
    assert_eq!(plan_row(&layout, "Widget"), (GenPlanCategory::Stale, false));
}

#[test]
fn missing_lockfile_is_a_distinct_stable_key() {
    // `cargo check` on an unresolvable dependency writes no Cargo.lock;
    // a fresh crate has none until its first check. Both must key
    // deterministically and differ from any hash.
    let root = tmp("gate-nolock");
    let layout = minimal_project(&root);
    let none = lockfile_hash(&layout).unwrap();
    assert_eq!(none, KEY_NONE);
    assert_eq!(lockfile_hash(&layout).unwrap(), none, "stable across calls");

    fs::write(root.join("Cargo.lock"), "# lock\n").unwrap();
    let some = lockfile_hash(&layout).unwrap();
    assert_ne!(some, KEY_NONE);
    assert!(some.starts_with("sha256:"), "{some}");

    // Three parts: the row's own input half, the lockfile, the profile
    // substrate. A key written before the profile half existed has two
    // parts and can never match one written now — every tree re-gates
    // once after the upgrade, which is the safe direction.
    assert_eq!(compose_gate_key(None, KEY_NONE, KEY_NONE), "none|none|none");
    let prof = profile_hash(&layout).unwrap();
    let basis = GateKeyBasis::compute(&layout).unwrap();
    assert_eq!(basis.lock, some);
    assert_eq!(basis.profile, prof);
    assert_eq!(
        basis.batch_key(),
        format!("{}|{some}|{prof}", basis.modules),
        "a batch row's input half is the module-set hash, never `none`"
    );
    assert_eq!(
        basis.module_key(None),
        format!("none|{some}|{prof}"),
        "a module whose prompt cannot be assembled keys on the fixed token"
    );
}

#[test]
fn toolchain_pin_file_joins_the_key() {
    let root = tmp("gate-toolchain");
    let layout = minimal_project(&root);
    fs::write(root.join("Cargo.lock"), "# lock\n").unwrap();
    let before = lockfile_hash(&layout).unwrap();
    fs::write(
        root.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.85\"\n",
    )
    .unwrap();
    let after = lockfile_hash(&layout).unwrap();
    assert_ne!(before, after, "a toolchain pin is a project fact and keys");
}

#[test]
fn workspace_member_keys_on_the_root_lockfile() {
    // A cargo workspace has ONE Cargo.lock, at the root; members have none.
    // The walk must find it, bounded by the workspace spec.toml that lists
    // the member.
    let ws = tmp("gate-ws");
    fs::write(
        ws.join("spec.toml"),
        r#"
spec_version = "1.0"

[project]
name = "ws-root"
profile = "rust-workspace"
language = "rust"

[workspace]
members = [{ path = "crates/app" }]
"#,
    )
    .unwrap();
    let member = ws.join("crates/app");
    let layout = minimal_project(&member);
    assert_eq!(
        lockfile_hash(&layout).unwrap(),
        KEY_NONE,
        "no lockfile anywhere yet"
    );

    fs::write(ws.join("Cargo.lock"), "# root lock v1\n").unwrap();
    let v1 = lockfile_hash(&layout).unwrap();
    assert_ne!(v1, KEY_NONE, "root lockfile found from the member");

    fs::write(ws.join("Cargo.lock"), "# root lock v2\n").unwrap();
    let v2 = lockfile_hash(&layout).unwrap();
    assert_ne!(v1, v2, "root lockfile change re-keys the member");
}

#[test]
fn walk_is_bounded_by_the_repository_root_when_no_workspace_spec() {
    // No workspace spec.toml above the project: the walk stops at the
    // nearest `.git`, and a lockfile beyond it is NOT the project's.
    let outer = tmp("gate-gitbound");
    fs::write(outer.join("Cargo.lock"), "# unrelated lock\n").unwrap();
    let repo = outer.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    let proj = repo.join("proj");
    let layout = minimal_project(&proj);
    assert_eq!(
        lockfile_hash(&layout).unwrap(),
        KEY_NONE,
        "the lockfile above .git must not be seen"
    );
    fs::write(repo.join("Cargo.lock"), "# repo lock\n").unwrap();
    assert_ne!(
        lockfile_hash(&layout).unwrap(),
        KEY_NONE,
        "the repo-root lockfile is seen"
    );
}

#[test]
fn record_gate_writes_a_batch_row_and_replaces_by_module_and_label() {
    let root = tmp("gate-rows");
    let layout = minimal_project(&root);
    unchanged_module(&root, &layout, "Widget");
    unchanged_module(&root, &layout, "Gadget");

    let rows = record_gate(
        &layout,
        &[
            "Widget".to_string(),
            "Gadget".to_string(),
            "Widget".to_string(),
        ],
        "cargo-check",
        false,
    )
    .unwrap();
    // Batch row first, then modules sorted + deduped.
    let modules: Vec<Option<&str>> = rows.iter().map(|r| r.module.as_deref()).collect();
    assert_eq!(modules, vec![None, Some("Gadget"), Some("Widget")]);

    let lineage = GenLineage::load(&layout.specdev_root).unwrap();
    assert_eq!(lineage.gates.len(), 3);
    assert!(!lineage.gate(None, "cargo-check").unwrap().pass);

    // A second result for the same (module, label) replaces, never appends.
    record_gate(&layout, &["Widget".to_string()], "cargo-check", true).unwrap();
    let lineage = GenLineage::load(&layout.specdev_root).unwrap();
    assert_eq!(lineage.gates.len(), 3, "replaced, not appended");
    assert!(lineage.gate(Some("Widget"), "cargo-check").unwrap().pass);
    assert!(
        lineage.gate(None, "cargo-check").unwrap().pass,
        "batch row replaced too"
    );
    assert!(
        !lineage.gate(Some("Gadget"), "cargo-check").unwrap().pass,
        "untouched"
    );

    // A different label is a different row.
    record_gate(&layout, &[], "next-build", true).unwrap();
    let lineage = GenLineage::load(&layout.specdev_root).unwrap();
    assert_eq!(lineage.gates.len(), 4);

    // gen-plan surfaces the batch state per label, sorted.
    let plan = run_gen_plan(&layout).unwrap();
    let labels: Vec<&str> = plan.batch.iter().map(|b| b.label.as_str()).collect();
    assert_eq!(labels, vec!["cargo-check", "next-build"]);
    assert!(
        plan.batch.iter().all(|b| b.current),
        "keys match the current lockfile"
    );
    assert!(plan.batch.iter().all(|b| b.pass));
}

#[test]
fn a_batch_row_goes_stale_when_any_module_input_moves() {
    // G2. A batch row keyed on the lockfile alone reads `current: true`
    // after any module regenerates, and Step 9 then says the tree builds
    // when it was never built with the new code. The module-set hash is
    // what closes that by construction, not by "one gate, run every time".
    let root = tmp("gate-batchstale");
    let layout = minimal_project(&root);
    unchanged_module(&root, &layout, "Widget");
    unchanged_module(&root, &layout, "Gadget");

    record_gate(&layout, &[], "next-build", true).unwrap();
    let plan = run_gen_plan(&layout).unwrap();
    assert_eq!(plan.batch.len(), 1);
    assert!(plan.batch[0].current, "recorded against today's tree");

    // Gadget's spec moves. Nothing about the lockfile changed.
    write_module(&root, "Gadget", "description = \"moved\"\n");
    let plan = run_gen_plan(&layout).unwrap();
    assert!(
        !plan.batch[0].current,
        "the tree that built is not the tree that is here now"
    );

    // A module that appears moves it too — the set is part of the key.
    record_gate(&layout, &[], "next-build", true).unwrap();
    assert!(run_gen_plan(&layout).unwrap().batch[0].current);
    write_module(&root, "Sprocket", "");
    assert!(
        !run_gen_plan(&layout).unwrap().batch[0].current,
        "a new module was never in the batch that built"
    );
}

#[test]
fn modules_hash_keys_an_unassemblable_module_on_the_fixed_token() {
    // A module whose prompt cannot be assembled is a state, not an error:
    // it must key deterministically, or every batch row would churn.
    let root = tmp("gate-moduleshash");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let a = modules_hash(&layout).unwrap();
    assert_eq!(a, modules_hash(&layout).unwrap(), "stable across calls");
    assert!(a.starts_with("sha256:"));

    // Break the module document. The hash still computes, and it moves —
    // "cannot be assembled" is itself a fact about the batch.
    fs::write(
        root.join("specs/modules/Widget.toml"),
        "name = \"Widget\"\nlayer = 4\n",
    )
    .unwrap();
    let b = modules_hash(&layout).unwrap();
    assert_ne!(a, b);
    assert_eq!(
        b,
        modules_hash(&layout).unwrap(),
        "and it is stable there too"
    );
}

#[test]
fn profile_hash_covers_the_registry_templates_recursively_and_paired_targets() {
    let root = tmp("gate-profilehash");
    let layout = minimal_project(&root);
    let registry_only = profile_hash(&layout).unwrap();
    assert!(registry_only.starts_with("sha256:"), "profile.yaml alone");

    // The registry itself keys: changing what the gate runs changes what
    // a pass row means.
    write_profile(
        &root,
        "gates:\n  - label: cargo-check\n    scope: module\n    steps: [[cargo, check]]\n",
    );
    let changed_registry = profile_hash(&layout).unwrap();
    assert_ne!(registry_only, changed_registry);

    // `templates/` is walked recursively: bootstrap copies flat today and
    // the hash must not assume it.
    fs::create_dir_all(root.join("specs/specdev/templates/a/b")).unwrap();
    fs::write(root.join("specs/specdev/templates/a/b/shim.tmpl"), "one\n").unwrap();
    let with_nested = profile_hash(&layout).unwrap();
    assert_ne!(
        changed_registry, with_nested,
        "a nested template joins the key"
    );
    fs::write(root.join("specs/specdev/templates/a/b/shim.tmpl"), "two\n").unwrap();
    assert_ne!(with_nested, profile_hash(&layout).unwrap());

    // A `paired_files` target joins only when it exists on disk: a
    // declared-but-absent config file is a state, not a hash input.
    write_profile(
        &root,
        "paired_files:\n  - template_file: templates/a/b/shim.tmpl\n    target: tsconfig.json\n\
         gates:\n  - label: cargo-check\n    scope: module\n    steps: [[cargo, check]]\n",
    );
    let declared_absent = profile_hash(&layout).unwrap();
    fs::write(root.join("tsconfig.json"), "{\"strict\": true}\n").unwrap();
    let present = profile_hash(&layout).unwrap();
    assert_ne!(declared_absent, present, "the file that exists is hashed");
    fs::write(root.join("tsconfig.json"), "{\"strict\": false}\n").unwrap();
    assert_ne!(present, profile_hash(&layout).unwrap(), "and its bytes key");
}

#[test]
fn a_symlinked_directory_under_templates_does_not_loop() {
    // A symlink back to an ancestor is a cycle. Proven by turning the
    // guard off: the walk then re-enters until macOS refuses the deep
    // path, and this assertion fails because the same template was
    // hashed once per level. A symlinked *file* cannot loop and is
    // hashed like any other.
    let root = tmp("gate-symlink");
    let layout = minimal_project(&root);
    let templates = root.join("specs/specdev/templates");
    fs::create_dir_all(templates.join("a")).unwrap();
    fs::write(templates.join("a/shim.tmpl"), "one\n").unwrap();
    let before = profile_hash(&layout).unwrap();

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&templates, templates.join("a/loop")).unwrap();
        // Terminates, and the cycle contributes nothing.
        assert_eq!(profile_hash(&layout).unwrap(), before);

        // A symlinked file still keys.
        fs::write(root.join("outside.tmpl"), "two\n").unwrap();
        std::os::unix::fs::symlink(root.join("outside.tmpl"), templates.join("linked.tmpl"))
            .unwrap();
        assert_ne!(profile_hash(&layout).unwrap(), before);
    }
}

#[test]
fn profile_hash_of_a_tree_with_no_profile_substrate_is_the_fixed_token() {
    let root = tmp("gate-noprofile");
    let layout = minimal_project_without_registry(&root);
    assert_eq!(profile_hash(&layout).unwrap(), KEY_NONE);
}

#[test]
fn a_failed_run_does_not_touch_modules_outside_the_run() {
    // #107, point 5 of the decision: modules NOT in the failed run keep their pass rows and
    // skip; the batch row is what says the tree does not build.
    let root = tmp("gate-outside");
    let layout = minimal_project(&root);
    unchanged_module(&root, &layout, "Widget");
    unchanged_module(&root, &layout, "Gadget");
    record_gate(
        &layout,
        &["Widget".to_string(), "Gadget".to_string()],
        "cargo-check",
        true,
    )
    .unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, true)
    );
    assert_eq!(
        plan_row(&layout, "Gadget"),
        (GenPlanCategory::Unchanged, true)
    );

    // Gadget regenerated alone and broke the build.
    record_gate(&layout, &["Gadget".to_string()], "cargo-check", false).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, true),
        "kept"
    );
    assert_eq!(
        plan_row(&layout, "Gadget"),
        (GenPlanCategory::Unchanged, false)
    );
    let plan = run_gen_plan(&layout).unwrap();
    assert_eq!(plan.batch.len(), 1);
    assert!(
        !plan.batch[0].pass,
        "the tree state is carried by the batch row"
    );
    assert!(plan.batch[0].current);
}

#[test]
fn lineage_written_before_gates_existed_loads_with_an_empty_table() {
    let root = tmp("gate-oldfile");
    let layout = minimal_project(&root);
    fs::write(
        gen_lineage_path(&layout.specdev_root),
        "[files.\"src/dsp.rs\"]\nmodule = \"Dsp\"\ngenerated_at = \"2026-06-12T10:00:00Z\"\ncontent_hash = \"sha256:abc\"\ninput_hash = \"sha256:in\"\n",
    )
    .unwrap();
    let lineage = GenLineage::load(&layout.specdev_root).unwrap();
    assert_eq!(lineage.files.len(), 1);
    assert!(lineage.gates.is_empty());
    // And a file with no gates serializes with no `gates` key at all.
    assert!(!lineage.to_toml_string().unwrap().contains("gates"));
}

#[test]
fn gates_table_is_byte_stable_regardless_of_insertion_order() {
    let mk = |order: &[(Option<&str>, &str)]| {
        let mut l = GenLineage::default();
        for (module, label) in order {
            l.upsert_gate(specdev_analyzer::gen_lineage::GateResult {
                module: module.map(str::to_string),
                label: (*label).to_string(),
                pass: true,
                key: "none|none".into(),
            });
        }
        l.to_toml_string().unwrap()
    };
    let a = mk(&[
        (Some("Z"), "cargo-check"),
        (None, "cargo-check"),
        (Some("A"), "next-build"),
    ]);
    let b = mk(&[
        (Some("A"), "next-build"),
        (Some("Z"), "cargo-check"),
        (None, "cargo-check"),
    ]);
    assert_eq!(a, b);
    assert!(
        !a.contains("generated_at"),
        "gate rows carry no timestamp:\n{a}"
    );
}

#[test]
fn record_gate_refuses_an_empty_label() {
    let root = tmp("gate-nolabel");
    let layout = minimal_project(&root);
    let err = record_gate(&layout, &[], "  ", true).unwrap_err();
    assert!(format!("{err:#}").contains("--label"), "{err:#}");
}

// ---------------------------------------------------------------------------
// The gate registry: `gates_declared` and the every-declared-gate rule.
// ---------------------------------------------------------------------------

#[test]
fn no_registry_reads_gates_declared_false_and_never_verified() {
    // The vacuous reading — "every declared gate passed" over zero gates —
    // must not appear: with nothing declared a pass row proves nothing.
    let root = tmp("registry-none");
    let layout = minimal_project_without_registry(&root);
    unchanged_module(&root, &layout, "Widget");
    record_gate(&layout, &["Widget".to_string()], "cargo-check", true).unwrap();
    let plan = run_gen_plan(&layout).unwrap();
    assert!(!plan.gates_declared);
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );
    // The batch row is still reported per label.
    assert_eq!(plan.batch.len(), 1);
    assert!(plan.batch[0].pass && plan.batch[0].current);

    // A profile that exists but declares no `gates:` is the same state.
    write_profile(&root, "");
    let plan = run_gen_plan(&layout).unwrap();
    assert!(!plan.gates_declared);
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );
}

#[test]
fn batch_only_registry_reads_gates_declared_false() {
    let root = tmp("registry-batch-only");
    let layout = minimal_project_without_registry(&root);
    write_profile(
        &root,
        "gates:\n  - label: build\n    scope: batch\n    steps: [[cargo, build]]\n",
    );
    unchanged_module(&root, &layout, "Widget");
    record_gate(&layout, &["Widget".to_string()], "build", true).unwrap();
    let plan = run_gen_plan(&layout).unwrap();
    assert!(
        !plan.gates_declared,
        "a batch gate does not make `verified` computable"
    );
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );
}

#[test]
fn verified_needs_a_current_pass_row_from_every_module_scope_gate() {
    let root = tmp("registry-two");
    let layout = minimal_project_without_registry(&root);
    write_profile(
        &root,
        "gates:\n  - label: fast\n    scope: module\n    steps: [[pnpm, exec, tsc, --noEmit]]\n  - label: lint\n    scope: module\n    steps: [[pnpm, exec, eslint]]\n  - label: build\n    scope: batch\n    steps: [[pnpm, exec, next, build]]\n",
    );
    unchanged_module(&root, &layout, "Widget");
    let plan = run_gen_plan(&layout).unwrap();
    assert!(plan.gates_declared);
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );

    // One of two declared module gates passed: not verified.
    record_gate(&layout, &["Widget".to_string()], "fast", true).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );

    // The batch gate passing changes nothing on the module axis.
    record_gate(&layout, &[], "build", true).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );

    // Both module gates passed at the current key: verified.
    record_gate(&layout, &["Widget".to_string()], "lint", true).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, true)
    );

    // One of them fails on a later run: back to unverified, still unchanged.
    record_gate(&layout, &["Widget".to_string()], "lint", false).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );
}

#[test]
fn a_pass_row_under_an_undeclared_label_does_not_count() {
    // A label rename in the registry: rows under the old label are dead
    // (the runner prunes them later); they must not satisfy the new label.
    let root = tmp("registry-rename");
    let layout = minimal_project(&root);
    unchanged_module(&root, &layout, "Widget");
    record_gate(&layout, &["Widget".to_string()], "cargo-check", true).unwrap();
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, true)
    );

    write_profile(
        &root,
        "gates:\n  - label: check\n    scope: module\n    steps: [[cargo, check]]\n",
    );
    let plan = run_gen_plan(&layout).unwrap();
    assert!(plan.gates_declared);
    assert_eq!(
        plan_row(&layout, "Widget"),
        (GenPlanCategory::Unchanged, false)
    );
}

#[test]
fn gates_declared_is_in_the_yaml_report() {
    let root = tmp("registry-yaml");
    let layout = minimal_project(&root);
    write_module(&root, "Widget", "");
    let yaml = run_gen_plan(&layout).unwrap().to_yaml().unwrap();
    assert!(yaml.starts_with("gates_declared: true\n"), "{yaml}");
    write_profile(&root, "");
    let yaml = run_gen_plan(&layout).unwrap().to_yaml().unwrap();
    assert!(yaml.starts_with("gates_declared: false\n"), "{yaml}");
}

#[test]
fn duplicate_gate_labels_are_refused_at_profile_load() {
    let root = tmp("registry-dup");
    let layout = minimal_project_without_registry(&root);
    write_profile(
        &root,
        "gates:\n  - label: check\n    scope: module\n    steps: [[cargo, check]]\n  - label: check\n    scope: batch\n    steps: [[cargo, build]]\n",
    );
    let err = run_gen_plan(&layout).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("declares label `check` twice"), "{msg}");
}

#[test]
fn registry_schema_refuses_empty_steps_and_bad_scope() {
    let root = tmp("registry-schema");
    let layout = minimal_project_without_registry(&root);
    for bad in [
        "gates:\n  - label: check\n    scope: module\n    steps: []\n",
        "gates:\n  - label: check\n    scope: module\n    steps: [[]]\n",
        "gates:\n  - label: check\n    scope: always\n    steps: [[cargo]]\n",
        "gates:\n  - label: \"has space\"\n    scope: module\n    steps: [[cargo]]\n",
        "gates:\n  - label: check\n    steps: [[cargo]]\n",
        "gates:\n  - label: check\n    scope: module\n    steps: [\"cargo check\"]\n",
    ] {
        write_profile(&root, bad);
        let err = run_gen_plan(&layout).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("MANIFEST-001"), "{bad}\n{msg}");
    }
}
