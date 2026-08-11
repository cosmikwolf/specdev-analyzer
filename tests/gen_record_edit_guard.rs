//! `gen-record` must not launder a hand-edit into `clean`.
//!
//! Recording is what makes a file `clean`, so an unguarded record after an
//! edit makes that edit the new baseline and `gen-drift` can never surface it
//! again. This was observed in the field: a real project reported 8/8 `clean`
//! while two generated files carried hand edits, because every `gen-drift` in
//! that session was followed immediately by a `gen-record`.
//!
//! The guard is scoped to the *requested* files. Recording file A while an
//! unrelated file B is edited must still succeed — otherwise one stale edit
//! anywhere blocks every subsequent emission.

use std::fs;
use std::path::PathBuf;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    gen_lineage::{
        record_files, record_files_accepting_edits, run_gen_drift, GenFileCategory,
    },
    layout::LayoutContext,
};

fn full_plugin() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn fresh(tag: &str) -> (PathBuf, LayoutContext) {
    let ws = std::env::temp_dir().join(format!(
        "specdev-gen-record-guard-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&ws);
    let target = ws.join("proj");
    fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "mylib".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

/// Write a file under the project and record it as generated.
fn emit_and_record(target: &PathBuf, layout: &LayoutContext, rel: &str, body: &str) -> PathBuf {
    let p = target.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, body).unwrap();
    record_files(layout, None, &[p.clone()], "2026-01-01T00:00:00Z").unwrap();
    p
}

#[test]
fn recording_an_edited_file_is_refused() {
    let (target, layout) = fresh("refuse");
    let f = emit_and_record(&target, &layout, "src/lib.rs", "// generated\n");

    // Hand-edit it. gen-drift must now see it.
    fs::write(&f, "// generated\n// hand edit\n").unwrap();
    let report = run_gen_drift(&layout).unwrap();
    assert!(
        report
            .files
            .iter()
            .any(|d| d.category == GenFileCategory::Edited),
        "precondition: the edit must be visible as drift: {:#?}",
        report.files
    );

    // The unguarded re-record is what used to make it `clean` again.
    let err = record_files(&layout, None, &[f.clone()], "2026-01-02T00:00:00Z")
        .expect_err("re-recording an edited file must be refused");
    let msg = format!("{err:#}");
    assert!(msg.contains("src/lib.rs"), "error must name the file: {msg}");
    assert!(
        msg.contains("--accept-edits"),
        "error must name the escape hatch: {msg}"
    );

    // And the baseline must be untouched — the edit is still visible.
    let after = run_gen_drift(&layout).unwrap();
    assert!(
        after
            .files
            .iter()
            .any(|d| d.category == GenFileCategory::Edited),
        "a refused record must not move the baseline: {:#?}",
        after.files
    );
}

#[test]
fn accept_edits_records_deliberately() {
    let (target, layout) = fresh("accept");
    let f = emit_and_record(&target, &layout, "src/lib.rs", "// generated\n");
    fs::write(&f, "// generated\n// kept edit\n").unwrap();

    record_files_accepting_edits(&layout, None, &[f], "2026-01-02T00:00:00Z")
        .expect("--accept-edits must record");

    assert!(
        run_gen_drift(&layout).unwrap().is_clean(),
        "an accepted edit becomes the new baseline"
    );
}

/// The common path must not regress: a fresh emission records freely.
#[test]
fn recording_a_clean_or_new_file_still_works() {
    let (target, layout) = fresh("clean");
    let a = emit_and_record(&target, &layout, "src/a.rs", "// a\n");

    // Re-recording an unchanged file is fine.
    record_files(&layout, None, &[a], "2026-01-02T00:00:00Z")
        .expect("re-recording a clean file must succeed");

    // A brand-new file with no lineage entry is fine.
    let b = target.join("src/b.rs");
    fs::write(&b, "// b\n").unwrap();
    record_files(&layout, None, &[b], "2026-01-02T00:00:00Z")
        .expect("recording a new file must succeed");

    assert!(run_gen_drift(&layout).unwrap().is_clean());
}

/// The guard is scoped to the requested files. An edit to an unrelated file
/// must not block recording a different one, or a single stale edit anywhere
/// would wedge every later emission.
#[test]
fn an_unrelated_edited_file_does_not_block_recording() {
    let (target, layout) = fresh("scoped");
    let a = emit_and_record(&target, &layout, "src/a.rs", "// a\n");
    let b = emit_and_record(&target, &layout, "src/b.rs", "// b\n");

    // Edit A, then record B.
    fs::write(&a, "// a\n// edited\n").unwrap();
    record_files(&layout, None, &[b], "2026-01-02T00:00:00Z")
        .expect("recording B must succeed while only A is edited");

    // A is still reported as edited — recording B did not launder it.
    let report = run_gen_drift(&layout).unwrap();
    let a_entry = report
        .files
        .iter()
        .find(|d| d.path.ends_with("a.rs"))
        .expect("a.rs must still be tracked");
    assert_eq!(a_entry.category, GenFileCategory::Edited);
}
