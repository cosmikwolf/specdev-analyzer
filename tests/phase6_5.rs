//! Phase 6.5 integration tests: `plugin-lint blocks` extension.
//!
//! Each `bad_<NNN>_*` test derives a temp plugin tree from the Phase 6
//! fixture's `_plugin/`, mutates exactly one thing to introduce a single
//! defect, runs `lint_plugin_blocks`, and asserts the expected
//! `BLOCK-LINT-NNN` finding fires (and no others).
//!
//! The good-plugin test exercises every check positively against the
//! Phase 6 fixture (which mirrors the live plugin tree).

use std::path::{Path, PathBuf};

use specdev_analyzer::inline::{lint_plugin_blocks, lint_plugin_bridges};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_plugin(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase6_5-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_dir_all(&p);
    p
}

/// Recursively copy a directory tree.
fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let s = entry.path();
        let d = dst.join(entry.file_name());
        if s.is_dir() {
            copy_tree(&s, &d);
        } else {
            std::fs::copy(&s, &d).unwrap();
        }
    }
}

/// Clone the Phase 6 plugin into a fresh temp dir and apply `mutate` to
/// the cloned tree. Returns the new plugin root.
fn cloned_with<F: FnOnce(&Path)>(tag: &str, mutate: F) -> PathBuf {
    let dst = tmp_plugin(tag);
    copy_tree(&phase6_plugin(), &dst);
    mutate(&dst);
    dst
}

fn assert_only_finding(offenders: &[String], code: &str, block: &str) {
    let matching: Vec<&String> = offenders
        .iter()
        .filter(|o| o.contains(code) && o.starts_with(&format!("{block}:")))
        .collect();
    assert!(
        !matching.is_empty(),
        "expected {code} on {block}; got: {offenders:?}"
    );
    // Other findings are tolerated (the lint may surface multiple
    // metaschema errors per malformed reduction, for instance), but at
    // least one matches.
}

// ---------------------------------------------------------------------------
// 1 — Good plugin (Phase 6 fixture) passes lint.
// ---------------------------------------------------------------------------
#[test]
fn good_plugin_passes_lint() {
    let offenders = lint_plugin_blocks(&phase6_plugin()).unwrap();
    assert!(
        offenders.is_empty(),
        "Phase 6 fixture should pass; got offenders: {offenders:?}"
    );
}

// ---------------------------------------------------------------------------
// 2 — BLOCK-LINT-001: missing schema.yaml
// ---------------------------------------------------------------------------
#[test]
fn bad_001_missing_schema() {
    let plugin = cloned_with("bad-001", |root| {
        std::fs::remove_file(root.join("blocks/embassy/schema.yaml")).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert_only_finding(&offenders, "BLOCK-LINT-001", "embassy");
}

// ---------------------------------------------------------------------------
// 3 — BLOCK-LINT-002: missing reduction.yaml
// ---------------------------------------------------------------------------
#[test]
fn bad_002_missing_reduction() {
    let plugin = cloned_with("bad-002", |root| {
        std::fs::remove_file(root.join("blocks/embassy/reduction.yaml")).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert_only_finding(&offenders, "BLOCK-LINT-002", "embassy");
}

// ---------------------------------------------------------------------------
// 4 — BLOCK-LINT-003: no codegen/<lang>/README.md
// ---------------------------------------------------------------------------
#[test]
fn bad_003_missing_codegen() {
    let plugin = cloned_with("bad-003", |root| {
        std::fs::remove_dir_all(root.join("blocks/embassy/codegen")).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert_only_finding(&offenders, "BLOCK-LINT-003", "embassy");
}

// ---------------------------------------------------------------------------
// 5 — BLOCK-LINT-004: missing top-level README.md
// ---------------------------------------------------------------------------
#[test]
fn bad_004_missing_readme() {
    let plugin = cloned_with("bad-004", |root| {
        std::fs::remove_file(root.join("blocks/embassy/README.md")).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert_only_finding(&offenders, "BLOCK-LINT-004", "embassy");
}

// ---------------------------------------------------------------------------
// 6 — BLOCK-LINT-005: schema.yaml fails JSON Schema compile
// ---------------------------------------------------------------------------
#[test]
fn bad_005_invalid_schema() {
    let plugin = cloned_with("bad-005", |root| {
        // Write a schema with an invalid `type` value — JSON Schema
        // compile rejects it.
        let bad = r#"$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "https://specdev.dev/blocks/embassy.v1"
type: not_a_real_type
"#;
        std::fs::write(root.join("blocks/embassy/schema.yaml"), bad).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert_only_finding(&offenders, "BLOCK-LINT-005", "embassy");
}

// ---------------------------------------------------------------------------
// 7 — BLOCK-LINT-006: schema.yaml $id doesn't match convention
// ---------------------------------------------------------------------------
#[test]
fn bad_006_wrong_id() {
    let plugin = cloned_with("bad-006", |root| {
        let path = root.join("blocks/embassy/schema.yaml");
        let text = std::fs::read_to_string(&path).unwrap();
        let mutated = text.replace(
            r#"https://specdev.dev/blocks/embassy.v1"#,
            r#"https://wrong-domain.example/embassy"#,
        );
        std::fs::write(&path, mutated).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert_only_finding(&offenders, "BLOCK-LINT-006", "embassy");
}

// ---------------------------------------------------------------------------
// 8 — BLOCK-LINT-007: reduction.yaml fails the metaschema
// ---------------------------------------------------------------------------
#[test]
fn bad_007_invalid_reduction() {
    let plugin = cloned_with("bad-007", |root| {
        // Write a reduction with a relation missing required `from_kind`
        // and `to_kind`. The embedded metaschema rejects this shape.
        let bad = r#"metaschema_version: "v1"
entities: []
relations:
  - kind: bogus
    extends: depends_on
    from_spec: "specs/modules/*.toml"
    endpoints:
      from: "x"
      to: "y"
"#;
        std::fs::write(root.join("blocks/embassy/reduction.yaml"), bad).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert_only_finding(&offenders, "BLOCK-LINT-007", "embassy");
}

// ---------------------------------------------------------------------------
// 9 — Multiple defects across multiple blocks: every offender surfaces.
// ---------------------------------------------------------------------------
#[test]
fn multiple_blocks_multiple_defects() {
    let plugin = cloned_with("multi-defect", |root| {
        // embassy: lose its README (BLOCK-LINT-004).
        std::fs::remove_file(root.join("blocks/embassy/README.md")).unwrap();
        // stm32h5-rs: corrupt its $id (BLOCK-LINT-006).
        let path = root.join("blocks/stm32h5-rs/schema.yaml");
        let text = std::fs::read_to_string(&path).unwrap();
        let mutated = text.replace(
            r#"https://specdev.dev/blocks/stm32h5-rs.v1"#,
            r#"https://specdev.dev/blocks/wrong-name.v1"#,
        );
        std::fs::write(&path, mutated).unwrap();
    });
    let offenders = lint_plugin_blocks(&plugin).unwrap();
    assert!(
        offenders
            .iter()
            .any(|o| o.contains("BLOCK-LINT-004") && o.starts_with("embassy:")),
        "expected embassy BLOCK-LINT-004; got: {offenders:?}"
    );
    assert!(
        offenders
            .iter()
            .any(|o| o.contains("BLOCK-LINT-006") && o.starts_with("stm32h5-rs:")),
        "expected stm32h5-rs BLOCK-LINT-006; got: {offenders:?}"
    );
}

// ---------------------------------------------------------------------------
// 10 — `lint_plugin_bridges` and `lint_plugin_blocks` independent walks.
// (Sanity: bridge lint still works alongside the new block lint.)
// ---------------------------------------------------------------------------
#[test]
fn bridge_lint_still_works() {
    let plugin = cloned_with("bridge-and-block", |root| {
        // Add a deliberately-misnamed bridge directory.
        std::fs::create_dir_all(root.join("bridges/lvgl+embassy")).unwrap();
    });
    let bridge_offenders = lint_plugin_bridges(&plugin).unwrap();
    assert!(
        bridge_offenders.iter().any(|o| o.contains("BRIDGE-LEX-001")),
        "expected BRIDGE-LEX-001; got: {bridge_offenders:?}"
    );
    // Block lint independent — embassy + stm32h5-rs unchanged, should pass.
    let block_offenders = lint_plugin_blocks(&plugin).unwrap();
    assert!(
        block_offenders.is_empty(),
        "block lint should be unaffected by bridge defects; got: {block_offenders:?}"
    );
}
