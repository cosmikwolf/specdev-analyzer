//! Integration tests for the gen-lineage tripwire: `gen-record` machinery
//! (`gen_lineage::record_files`) + the `report gen-drift` classifier
//! (`gen_lineage::run_gen_drift`).

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::gen_lineage::{
    gen_lineage_path, record_files, run_gen_drift, GenFileCategory, GenLineage,
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

#[test]
fn re_record_returns_edited_file_to_clean() {
    let root = tmp("rerecord");
    let layout = minimal_project(&root);
    let src = root.join("src/widget.rs");
    fs::write(&src, "pub fn widget() {}\n").unwrap();
    record_files(&layout, Some("Widget"), &[src.clone()], TS).unwrap();

    fs::write(&src, "pub fn widget() { /* regenerated */ }\n").unwrap();
    assert!(!run_gen_drift(&layout).unwrap().is_clean());

    record_files(&layout, Some("Widget"), &[src], "2026-06-12T11:00:00Z").unwrap();
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
