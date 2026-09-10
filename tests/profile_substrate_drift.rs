//! The profile substrate is drift-tracked (#109).
//!
//! `drift` used to answer `units: []  invariants: []` for any project with
//! no inlined blocks — a confident clean over the schemas, templates and
//! `profile.yaml` that `bootstrap` had scaffolded and never hashed.
//! Measured on `dogfood-flutter-hello/rust`: clean report, while the
//! inlined `architecture.schema.yaml` was missing `external_deps` and that
//! omission was a hard `SCHEMA-ARCH` error blocking `manifest --write`.
//!
//! Coverage:
//!   1. `bootstrap` records every profile file, hashed POST-substitution.
//!   2. A tree with no `[profile_files]` reports `not_recorded` — which is
//!      not `clean`, and does not pretend to be.
//!   3. A hand-edited file is `LocallyEdited`.
//!   4. A moved plugin template is `PluginMoved`.
//!   5. Inlining a block leaves `module.schema.yaml` `Clean`: the recorded
//!      hash excludes the `allOf` array `inline` appends to.
//!   6. `migrate --refresh` rewrites a stale untouched file, declines an
//!      edited one, moves the base, and is idempotent.
//!   7. `migrate --refresh` on an unrecorded tree refuses rather than
//!      guessing which files are safe to overwrite.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    drift::{self, Comparison, FileCategory, ProfileDriftStatus},
    inline::run_inline_block,
    layout::LayoutContext,
    lineage::Lineage,
    migrate,
};

const MODULE_SCHEMA: &str = "specs/specdev/schemas/module.schema.yaml";
const ARCH_SCHEMA: &str = "specs/specdev/schemas/architecture.schema.yaml";

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/specs/full_plugin/_plugin")
}

fn minimal_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/specs/minimal_plugin/_plugin")
}

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-profile-substrate-{tag}-{}-{}",
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

fn boot(plugin: &Path, profile: &str, language: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let target = tmp(tag).join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        plugin,
        &BootstrapInputs {
            profile: profile.into(),
            language: language.into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .expect("bootstrap");
    let layout = LayoutContext::discover(&target, None).expect("discover");
    (target, layout)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let (from, to) = (entry.path(), dst.join(entry.file_name()));
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn category_of(report: &drift::DriftReport, path: &str) -> FileCategory {
    let p = report.profile.as_ref().expect("profile section present");
    p.files
        .iter()
        .find(|f| f.path == path)
        .unwrap_or_else(|| {
            panic!(
                "no profile row for {path}; rows: {:?}",
                p.files.iter().map(|f| &f.path).collect::<Vec<_>>()
            )
        })
        .category
}

// ---------------------------------------------------------------------------
// 1. Bootstrap records the substrate, hashed as written.
// ---------------------------------------------------------------------------
#[test]
fn bootstrap_records_profile_files_post_substitution() {
    let (target, layout) = boot(&full_plugin(), "library-api", "rust", "record");
    let lineage = Lineage::load(&layout.specdev_root).unwrap();
    assert!(
        lineage.records_profile_files(),
        "bootstrap must record the profile substrate"
    );
    for key in ["spec.toml", MODULE_SCHEMA, ARCH_SCHEMA] {
        assert!(
            lineage.profile_files.contains_key(key),
            "{key} must be recorded; got {:?}",
            lineage.profile_files.keys().collect::<Vec<_>>()
        );
    }

    // Post-substitution is the whole point: `module.schema.yaml` carries
    // `{{language}}`, and hashing the template text would leave the file
    // reporting drift forever.
    let on_disk = std::fs::read_to_string(target.join(MODULE_SCHEMA)).unwrap();
    assert!(
        !on_disk.contains("{{language}}"),
        "bootstrap must substitute the language token"
    );
    let recorded = &lineage.profile_files[MODULE_SCHEMA];
    let recomputed = bootstrap::profile_file_hash(MODULE_SCHEMA, on_disk.as_bytes()).unwrap();
    assert_eq!(
        recorded, &recomputed,
        "recorded hash must be of the file as written"
    );
}

// ---------------------------------------------------------------------------
// 2. A pre-#109 tree says so. Silence is not cleanliness.
// ---------------------------------------------------------------------------
#[test]
fn tree_without_recorded_substrate_reports_not_recorded() {
    let (_t, layout) = boot(&full_plugin(), "library-api", "rust", "unrecorded");
    // Simulate a project bootstrapped before the field existed.
    let mut lineage = Lineage::load(&layout.specdev_root).unwrap();
    lineage.profile_files.clear();
    lineage.save(&layout.specdev_root).unwrap();

    let report = drift::run_drift(&layout, &full_plugin()).unwrap();
    let p = report.profile.as_ref().expect("profile section present");
    assert_eq!(p.status, ProfileDriftStatus::NotRecorded);
    assert!(p.files.is_empty(), "nothing can be classified");
    assert!(
        report.profile_substrate_unrecorded(),
        "the report must be able to say it cannot see this far"
    );
    // An unseen substrate is not a dirty one; existing consumers of
    // is_clean() must not flip for every pre-#109 project.
    assert!(report.is_clean(), "unrecorded must not read as dirty");
}

// ---------------------------------------------------------------------------
// 3 + 4. The two single-sided categories.
// ---------------------------------------------------------------------------
#[test]
fn hand_edited_profile_file_is_locally_edited() {
    let (target, layout) = boot(&full_plugin(), "library-api", "rust", "local");
    let path = target.join(ARCH_SCHEMA);
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n# my own note\n");
    std::fs::write(&path, text).unwrap();

    let report = drift::run_drift(&layout, &full_plugin()).unwrap();
    assert_eq!(
        category_of(&report, ARCH_SCHEMA),
        FileCategory::LocallyEdited
    );
    assert!(!report.is_clean());
}

#[test]
fn moved_plugin_template_is_plugin_moved() {
    let (_t, layout) = boot(&full_plugin(), "library-api", "rust", "moved");

    // The plugin gains a key after this project was bootstrapped — #109's
    // measured shape (`architecture.schema.yaml` grew `external_deps`).
    let synthetic = tmp("moved-plugin").join("_plugin");
    copy_dir_recursive(&full_plugin(), &synthetic).unwrap();
    let upstream = synthetic.join("profiles/library-api/schemas/architecture.schema.yaml");
    let mut text = std::fs::read_to_string(&upstream).unwrap();
    text.push_str("\n# a key added upstream\n");
    std::fs::write(&upstream, text).unwrap();

    let report = drift::run_drift(&layout, &synthetic).unwrap();
    assert_eq!(category_of(&report, ARCH_SCHEMA), FileCategory::PluginMoved);
}

// ---------------------------------------------------------------------------
// 5. The aggregator's recorded hash is invariant under `inline`.
//
//    `inline` appends a `$ref` to `module.schema.yaml`'s `allOf` and
//    re-emits the document. That is the analyzer's own write, not the
//    user's, and `drift` reports it under `units:`. If the recorded hash
//    covered the whole file, every project with one inlined block would
//    report its aggregator as edited forever.
// ---------------------------------------------------------------------------
#[test]
fn inlining_a_block_leaves_the_aggregator_clean() {
    let (target, layout) = boot(&minimal_plugin(), "firmware-layered", "rust", "aggregator");
    let before = std::fs::read(target.join(MODULE_SCHEMA)).unwrap();

    run_inline_block(&layout, &minimal_plugin(), "embassy").expect("inline embassy");

    let after = std::fs::read(target.join(MODULE_SCHEMA)).unwrap();
    assert_ne!(before, after, "inline must actually rewrite the aggregator");

    let report = drift::run_drift(&layout, &minimal_plugin()).unwrap();
    assert_eq!(
        category_of(&report, MODULE_SCHEMA),
        FileCategory::Clean,
        "the analyzer's own $ref append is not user drift"
    );
    // And the report says the comparison was partial, rather than leaving
    // that as a rule the reader has to already know.
    let p = report.profile.as_ref().unwrap();
    let row = p.files.iter().find(|f| f.path == MODULE_SCHEMA).unwrap();
    assert_eq!(row.comparison, Comparison::ExcludesUnitRefs);
    for other in p.files.iter().filter(|f| f.path != MODULE_SCHEMA) {
        assert_eq!(
            other.comparison,
            Comparison::Full,
            "{} should be compared whole",
            other.path
        );
    }
}

// ---------------------------------------------------------------------------
// 6. `migrate --refresh`.
// ---------------------------------------------------------------------------
#[test]
fn refresh_rewrites_stale_untouched_and_declines_edited() {
    let (target, layout) = boot(&full_plugin(), "library-api", "rust", "refresh");

    // The plugin moves two files; the user has edited one of them.
    let synthetic = tmp("refresh-plugin").join("_plugin");
    copy_dir_recursive(&full_plugin(), &synthetic).unwrap();
    for rel in [
        "profiles/library-api/schemas/architecture.schema.yaml",
        "profiles/library-api/schemas/requirements.schema.yaml",
    ] {
        let p = synthetic.join(rel);
        let mut t = std::fs::read_to_string(&p).unwrap();
        t.push_str("\n# a key added upstream\n");
        std::fs::write(&p, t).unwrap();
    }
    let edited = target.join(ARCH_SCHEMA);
    let mut t = std::fs::read_to_string(&edited).unwrap();
    t.push_str("\n# my own note\n");
    std::fs::write(&edited, t).unwrap();

    let report = migrate::run_migrate(&layout, &synthetic, false, true).unwrap();
    let refreshed: Vec<String> = report
        .profile_files_refreshed
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    let kept: Vec<String> = report
        .profile_files_locally_edited
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        refreshed.contains(&"specs/specdev/schemas/requirements.schema.yaml".to_string()),
        "an untouched stale file must be refreshed; got {refreshed:?}"
    );
    assert!(
        kept.contains(&ARCH_SCHEMA.to_string()),
        "an edited file must be declined, not overwritten; got {kept:?}"
    );
    assert!(
        std::fs::read_to_string(&edited)
            .unwrap()
            .contains("my own note"),
        "the user's edit must survive refresh"
    );

    // The base moved with the write. Without this the file drift just made
    // clean would report `Both` — a write that does not re-hash leaves the
    // recorded base wrong on both sides.
    let after = drift::run_drift(&layout, &synthetic).unwrap();
    assert_eq!(
        category_of(&after, "specs/specdev/schemas/requirements.schema.yaml"),
        FileCategory::Clean,
        "refresh must move the recorded hash it invalidated"
    );

    // Idempotent: nothing left to do.
    let again = migrate::run_migrate(&layout, &synthetic, false, true).unwrap();
    assert!(
        again.profile_files_refreshed.is_empty(),
        "second refresh must be a no-op; got {:?}",
        again.profile_files_refreshed
    );
}

// ---------------------------------------------------------------------------
// 7. Refresh needs the recorded hashes to prove a file is safe to rewrite.
// ---------------------------------------------------------------------------
#[test]
fn refresh_without_recorded_hashes_refuses() {
    let (target, layout) = boot(&full_plugin(), "library-api", "rust", "refresh-blind");
    let mut lineage = Lineage::load(&layout.specdev_root).unwrap();
    lineage.profile_files.clear();
    lineage.save(&layout.specdev_root).unwrap();

    let synthetic = tmp("blind-plugin").join("_plugin");
    copy_dir_recursive(&full_plugin(), &synthetic).unwrap();
    let upstream = synthetic.join("profiles/library-api/schemas/architecture.schema.yaml");
    let mut t = std::fs::read_to_string(&upstream).unwrap();
    t.push_str("\n# a key added upstream\n");
    std::fs::write(&upstream, t).unwrap();
    let before = std::fs::read(target.join(ARCH_SCHEMA)).unwrap();

    let report = migrate::run_migrate(&layout, &synthetic, false, true).unwrap();
    assert!(
        report.profile_refresh_unavailable,
        "refresh must say it cannot run rather than guess"
    );
    assert!(report.profile_files_refreshed.is_empty());
    assert_eq!(
        std::fs::read(target.join(ARCH_SCHEMA)).unwrap(),
        before,
        "with no recorded hash, nothing may be overwritten"
    );
}

// ---------------------------------------------------------------------------
// 8. A freshly bootstrapped, untouched tree has nothing for refresh to say.
//
//    Regression guard. The recorded value for a co-owned aggregator is a
//    NORMALIZED hash; comparing it with plain `content_hash` mismatches
//    every time, and the first run of this against a clean tree reported
//    `module.schema.yaml` as "you changed them". Two hash meanings, one
//    comparison site that did not know it.
// ---------------------------------------------------------------------------
#[test]
fn refresh_on_untouched_tree_reports_nothing() {
    let (_t, layout) = boot(&full_plugin(), "library-api", "rust", "refresh-noop");
    let report = migrate::run_migrate(&layout, &full_plugin(), false, true).unwrap();
    assert!(
        report.profile_files_locally_edited.is_empty(),
        "an untouched tree has no user-edited files; got {:?}",
        report.profile_files_locally_edited
    );
    assert!(report.profile_files_refreshed.is_empty());
    assert!(report.profile_files_needs_manual.is_empty());
    assert!(!report.profile_refresh_unavailable);
}

// ---------------------------------------------------------------------------
// 9. Refresh must never rewrite the co-owned aggregator.
//
//    `module.schema.yaml` carries the `$ref` list `inline` appended; the
//    plugin's template does not. Writing the template back would silently
//    delete every inlined unit's schema reference. The move is reported as
//    MANUAL instead — detectable, not mechanically applicable.
// ---------------------------------------------------------------------------
#[test]
fn refresh_never_overwrites_the_co_owned_aggregator() {
    let (target, layout) = boot(&minimal_plugin(), "firmware-layered", "rust", "agg-manual");
    run_inline_block(&layout, &minimal_plugin(), "embassy").expect("inline embassy");
    let with_refs = std::fs::read_to_string(target.join(MODULE_SCHEMA)).unwrap();
    assert!(
        with_refs.contains("inlined/blocks/embassy.schema.yaml"),
        "precondition: the aggregator must hold the inlined $ref"
    );

    // The plugin's template gains a real property (#109's measured shape:
    // `module.schema.yaml` grew `source_file` upstream). A comment-only
    // change would be invisible, and correctly so — the normalized hash
    // re-emits through serde_yaml, which carries no comments.
    let synthetic = tmp("agg-plugin").join("_plugin");
    copy_dir_recursive(&minimal_plugin(), &synthetic).unwrap();
    let upstream = synthetic.join("profiles/firmware-layered/schemas/module.schema.yaml");
    let text = std::fs::read_to_string(&upstream).unwrap();
    let text = text.replacen(
        "properties:",
        "properties:\n  brand_new_upstream_key:\n    description: added upstream\n    type: string",
        1,
    );
    std::fs::write(&upstream, text).unwrap();

    let report = migrate::run_migrate(&layout, &synthetic, false, true).unwrap();
    let manual: Vec<String> = report
        .profile_files_needs_manual
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        manual.contains(&MODULE_SCHEMA.to_string()),
        "a moved aggregator template must be reported as manual; got {manual:?}"
    );

    // The decisive assertion: the $refs survived and the upstream key was
    // NOT applied. Losing the refs would break every inlined block's
    // schema resolution.
    let after = std::fs::read_to_string(target.join(MODULE_SCHEMA)).unwrap();
    assert!(
        after.contains("inlined/blocks/embassy.schema.yaml"),
        "refresh must not drop the $ref list"
    );
    assert!(
        !after.contains("brand_new_upstream_key"),
        "refresh must not rewrite a co-owned file at all"
    );

    // But drift still names it, so the staleness is not hidden.
    let d = drift::run_drift(&layout, &synthetic).unwrap();
    assert_eq!(category_of(&d, MODULE_SCHEMA), FileCategory::PluginMoved);
}
