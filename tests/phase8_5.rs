//! Phase 8.5 integration tests — F-004 closed-set CrateReference.source.
//!
//! Re-validates the dogfood scenario that surfaced the bug: a path-dep
//! between two library-api crates was silently dropped because the
//! schema accepted any string and the renderer had no case for it.
//!
//! The new behavior:
//!   * `{ path = "..." }`            renders `path = "..."`
//!   * `{ git = "...", rev = "..." }` renders `git`+`rev`
//!   * `"workspace"`                 renders `{ workspace = true }`
//!   * `"crates-io"`                 omits the source field
//!   * Any other shape (including the old loose `"path:..."` string)
//!     is rejected at validate time — no silent drops.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
    report::{Finding, Report},
    validate, Severity,
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase8_5-{tag}-{}-{}",
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

fn fresh_library(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn errors(report: &Report) -> Vec<&Finding> {
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect()
}

fn rule_codes(report: &Report) -> Vec<String> {
    let mut out: Vec<String> = report
        .findings()
        .iter()
        .map(|f| f.rule.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

// ---------------------------------------------------------------------------
// 1 — `{ path = "../sibling" }` validates and renders `path = "..."`.
//     This is the primary regression test for the zfm dogfood scenario.
// ---------------------------------------------------------------------------
#[test]
fn path_dep_renders_path_field() {
    let (target, layout) = fresh_library("path-dep");
    std::fs::write(
        target.join("specs/modules/Adapter.toml"),
        r#"name = "Adapter"
visibility = "pub"

[[external_deps]]
crate = "sibling-core"
version = "0.1.0"
default_features = false
source = { path = "../sibling-core" }
"#,
    )
    .unwrap();

    let validate_report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&validate_report).is_empty(),
        "path-dep should validate clean; got:\n{}",
        validate_report.render_plain()
    );

    let r = render(&layout).unwrap();
    assert!(
        r.text.contains("path = \"../sibling-core\""),
        "expected path field in rendered Cargo.toml; got:\n{}",
        r.text
    );
    assert!(
        r.text.contains("sibling-core = {"),
        "expected inline-table form; got:\n{}",
        r.text
    );
}

// ---------------------------------------------------------------------------
// 2 — `{ git = "...", rev = "..." }` validates and renders both fields.
// ---------------------------------------------------------------------------
#[test]
fn git_dep_with_rev_renders_both_fields() {
    let (target, layout) = fresh_library("git-rev");
    std::fs::write(
        target.join("specs/modules/Vendored.toml"),
        r#"name = "Vendored"
visibility = "pub"

[[external_deps]]
crate = "upstream-crate"
version = "0.5.0"
source = { git = "https://github.com/example/upstream", rev = "abc1234" }
"#,
    )
    .unwrap();

    let validate_report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&validate_report).is_empty(),
        "git-dep should validate clean; got:\n{}",
        validate_report.render_plain()
    );

    let r = render(&layout).unwrap();
    assert!(
        r.text.contains("git = \"https://github.com/example/upstream\""),
        "expected git field; got:\n{}",
        r.text
    );
    assert!(
        r.text.contains("rev = \"abc1234\""),
        "expected rev field; got:\n{}",
        r.text
    );
}

// ---------------------------------------------------------------------------
// 3 — `{ git = "...", tag = "..." }` renders git + tag.
// ---------------------------------------------------------------------------
#[test]
fn git_dep_with_tag_renders_tag_field() {
    let (target, layout) = fresh_library("git-tag");
    std::fs::write(
        target.join("specs/modules/Vendored.toml"),
        r#"name = "Vendored"
visibility = "pub"

[[external_deps]]
crate = "upstream-crate"
version = "0.5.0"
source = { git = "https://github.com/example/upstream", tag = "v0.5.0" }
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    assert!(errors(&report).is_empty());

    let r = render(&layout).unwrap();
    assert!(r.text.contains("git = \"https://github.com/example/upstream\""));
    assert!(r.text.contains("tag = \"v0.5.0\""));
    assert!(!r.text.contains("rev = "));
    assert!(!r.text.contains("branch = "));
}

// ---------------------------------------------------------------------------
// 4 — `source = "workspace"` validates and renders `{ workspace = true }`,
//     dropping version/features/default-features per Cargo's rules.
// ---------------------------------------------------------------------------
#[test]
fn workspace_dep_renders_workspace_true_only() {
    let (target, layout) = fresh_library("workspace");
    std::fs::write(
        target.join("specs/modules/Inherits.toml"),
        r#"name = "Inherits"
visibility = "pub"

[[external_deps]]
crate = "shared-helper"
version = "0.1.0"
features = ["serde"]
default_features = false
source = "workspace"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "workspace source should validate clean; got:\n{}",
        report.render_plain()
    );

    let r = render(&layout).unwrap();
    assert!(
        r.text.contains("shared-helper = { workspace = true }"),
        "expected `workspace = true` shape; got:\n{}",
        r.text
    );
    // Cargo's workspace inheritance forbids version/features/default-features
    // at the member-dep site — those come from the workspace table.
    let line = r
        .text
        .lines()
        .find(|l| l.starts_with("shared-helper ="))
        .expect("shared-helper line present");
    assert!(!line.contains("version"), "workspace dep must omit version: {line}");
    assert!(!line.contains("features = ["), "workspace dep must omit features: {line}");
    assert!(
        !line.contains("default-features"),
        "workspace dep must omit default-features: {line}"
    );
}

// ---------------------------------------------------------------------------
// 5 — `source = "crates-io"` is the explicit default; rendered identically
//     to omitting source (no source field appears).
// ---------------------------------------------------------------------------
#[test]
fn crates_io_dep_omits_source_field() {
    let (target, layout) = fresh_library("crates-io");
    std::fs::write(
        target.join("specs/modules/Plain.toml"),
        r#"name = "Plain"
visibility = "pub"

[[external_deps]]
crate = "thiserror"
version = "1.0"
source = "crates-io"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    assert!(errors(&report).is_empty());

    let r = render(&layout).unwrap();
    assert!(
        r.text.contains("thiserror = \"1.0\""),
        "expected bare-version form when crates-io is the source; got:\n{}",
        r.text
    );
    let line = r
        .text
        .lines()
        .find(|l| l.starts_with("thiserror ="))
        .unwrap();
    assert!(!line.contains("source"), "no source key for crates-io: {line}");
}

// ---------------------------------------------------------------------------
// 6 — Legacy ad-hoc `"path:..."` string (the form that used to silently
//     pass validate and silently drop in cargo-toml) is now rejected at
//     schema-validate time. No silent data loss.
// ---------------------------------------------------------------------------
#[test]
fn legacy_path_string_rejected_at_validate() {
    let (target, _layout) = fresh_library("legacy-path-string");
    std::fs::write(
        target.join("specs/modules/Broken.toml"),
        r#"name = "Broken"
visibility = "pub"

[[external_deps]]
crate = "sibling-core"
version = "0.1.0"
source = "path:../sibling-core"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let codes = rule_codes(&report);
    assert!(
        codes.contains(&"SCHEMA".to_string()) || codes.contains(&"SCHEMA-MODULE".to_string()),
        "expected SCHEMA finding rejecting legacy path-string source; got: {:?}\n{}",
        codes,
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 7 — Unrecognized string sentinel (not "workspace" or "crates-io") rejected.
// ---------------------------------------------------------------------------
#[test]
fn unknown_source_string_rejected() {
    let (target, _layout) = fresh_library("unknown-string");
    std::fs::write(
        target.join("specs/modules/Broken.toml"),
        r#"name = "Broken"
visibility = "pub"

[[external_deps]]
crate = "demo"
version = "0.1.0"
source = "registry+https://example.com"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let codes = rule_codes(&report);
    assert!(
        codes.iter().any(|c| c.starts_with("SCHEMA")),
        "expected SCHEMA finding; got: {:?}",
        codes
    );
}

// ---------------------------------------------------------------------------
// 8 — Path-dep with `default_features = false` renders both keys.
//     Mirrors the exact zfm dogfood shape (sibling crate, no_std-leaning).
// ---------------------------------------------------------------------------
#[test]
fn path_dep_with_default_features_false_renders_both() {
    let (target, layout) = fresh_library("path-no-default");
    std::fs::write(
        target.join("specs/modules/Adapter.toml"),
        r#"name = "Adapter"
visibility = "pub"

[[external_deps]]
crate = "sibling-core"
version = "0.1.0"
default_features = false
source = { path = "../sibling-core" }
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    assert!(errors(&report).is_empty());

    let r = render(&layout).unwrap();
    let line = r
        .text
        .lines()
        .find(|l| l.starts_with("sibling-core ="))
        .expect("sibling-core present");
    assert!(line.contains("version = \"0.1.0\""), "version present: {line}");
    assert!(line.contains("default-features = false"), "default-features present: {line}");
    assert!(line.contains("path = \"../sibling-core\""), "path present: {line}");
}

// ---------------------------------------------------------------------------
// 9 — Object-shape `source = { path = ... }` rendered byte-stable across runs.
// ---------------------------------------------------------------------------
#[test]
fn path_dep_render_byte_stable() {
    let (target, layout) = fresh_library("path-stable");
    std::fs::write(
        target.join("specs/modules/Adapter.toml"),
        r#"name = "Adapter"
visibility = "pub"

[[external_deps]]
crate = "sibling-core"
version = "0.1.0"
source = { path = "../sibling-core" }
"#,
    )
    .unwrap();

    let a = render(&layout).unwrap().text;
    let b = render(&layout).unwrap().text;
    let c = render(&layout).unwrap().text;
    assert_eq!(a, b);
    assert_eq!(b, c);
}

// ---------------------------------------------------------------------------
// 10 — Path-dep + features array round-trips with deterministic ordering.
// ---------------------------------------------------------------------------
#[test]
fn path_dep_with_features_renders_all_fields() {
    let (target, layout) = fresh_library("path-features");
    std::fs::write(
        target.join("specs/modules/Adapter.toml"),
        r#"name = "Adapter"
visibility = "pub"

[[external_deps]]
crate = "sibling-core"
version = "0.1.0"
features = ["alloc", "serde"]
default_features = false
source = { path = "../sibling-core" }
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    assert!(errors(&report).is_empty());

    let r = render(&layout).unwrap();
    let line = r
        .text
        .lines()
        .find(|l| l.starts_with("sibling-core ="))
        .unwrap();
    assert!(line.contains("features = [\"alloc\", \"serde\"]"));
    assert!(line.contains("path = \"../sibling-core\""));
}
