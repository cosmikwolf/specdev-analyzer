//! Integration tests for the gen-lineage tripwire: `gen-record` machinery
//! (`gen_lineage::record_files`) + the `report gen-drift` classifier
//! (`gen_lineage::run_gen_drift`) + the `report gen-plan` incremental
//! classifier (`gen_lineage::run_gen_plan`).

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::gen_lineage::{
    gen_lineage_path, record_files, record_files_accepting_edits, run_gen_drift,
    run_gen_plan, GenFileCategory, GenLineage, GenPlanCategory,
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

/// Minimal project: spec.toml + specs/specdev/ + src/. Enough for
/// `LayoutContext::discover`; gen-lineage machinery needs nothing else.
fn minimal_project(root: &Path) -> LayoutContext {
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
    assert_eq!(entry.input_hash, None, "unassemblable module → no input_hash");
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

    record_files_accepting_edits(&layout, Some("Widget"), &[src], "2026-06-12T11:00:00Z")
        .unwrap();
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
