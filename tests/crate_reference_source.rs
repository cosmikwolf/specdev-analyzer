//! Closed-set `CrateReference.source` (F-004).
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
    layout::LayoutContext,
    report::{Finding, Report},
    validate, Severity,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-crate-reference-source-{tag}-{}-{}",
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
        &full_plugin(),
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

#[test]
fn registry_dep_without_version_still_rejected() {
    let (target, _layout) = fresh_library("registry-noversion");
    std::fs::write(
        target.join("specs/modules/Pinned.toml"),
        r#"name = "Pinned"
visibility = "pub"

[[external_deps]]
crate = "serde"
source = "crates-io"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors(&report);
    assert!(
        !errs.is_empty(),
        "a registry dep without a version must fail validation; got:\n{}",
        report.render_plain()
    );
    assert!(
        errs.iter().any(|f| f.message.contains("version")),
        "the error must name the missing version; got:\n{}",
        report.render_plain()
    );
}
