//! `items diff` deterministic detection.
//!
//! Coverage:
//!   * Spec items[] match source items by `(kind, name)`.
//!   * Items in spec but missing from source surface as `in_spec_only`.
//!   * Items in source but missing from spec surface as `in_source_only`.
//!   * `signature_hint` divergence flags signature_drift on the both[] pair.
//!   * Module with no source file emits all spec items as `in_spec_only`
//!     (pre-implementation modules).
//!   * Exit code: 0 when clean, 1 when drift present, 2 on hard error.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-items-diff-{tag}-{}-{}",
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

fn analyzer_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("specdev-analyzer")
}

struct CliResult {
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_items_diff(project: &Path, module: &str) -> CliResult {
    let out = Command::new(analyzer_bin())
        .arg("items")
        .arg("diff")
        .arg("--project")
        .arg(project)
        .arg("--module")
        .arg(module)
        .output()
        .expect("invoke items diff");
    CliResult {
        success: out.status.success(),
        exit_code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn bootstrap_lib(target: &Path) {
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.to_path_buf(),
        },
    )
    .unwrap();
}

#[test]
fn items_diff_clean_when_spec_matches_source() {
    let target = tmp_dir("clean");
    bootstrap_lib(&target);

    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render the frame."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();

    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn render() -> u32 { 0 }\n",
    )
    .unwrap();

    let r = run_items_diff(&target, "Foo");
    assert!(
        r.success,
        "expected exit 0 (clean); exit={:?} stderr={}",
        r.exit_code, r.stderr
    );
    assert!(r.stdout.contains("module: Foo"));
    assert!(r.stdout.contains("in_spec_only: []"));
    assert!(r.stdout.contains("in_source_only: []"));
}

#[test]
fn items_diff_reports_in_spec_only_when_source_missing_item() {
    let target = tmp_dir("spec-only");
    bootstrap_lib(&target);

    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render the frame."
satisfies = ["DEMO-1"]

[[items]]
name = "ghost"
kind = "fn"
description = "Spec-only function not yet implemented."
satisfies = ["DEMO-2"]
"#,
    )
    .unwrap();

    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn render() -> u32 { 0 }\n",
    )
    .unwrap();

    let r = run_items_diff(&target, "Foo");
    assert_eq!(r.exit_code, Some(1), "expected exit 1 (drift); stderr={}", r.stderr);
    assert!(r.stdout.contains("name: ghost"));
    // The both[] bucket should still contain `render`.
    assert!(r.stdout.contains("name: render"));
}

#[test]
fn items_diff_reports_in_source_only_for_uncovered_items() {
    let target = tmp_dir("source-only");
    bootstrap_lib(&target);

    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render the frame."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();

    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn render() -> u32 { 0 }\npub fn helper() -> u32 { 1 }\n",
    )
    .unwrap();

    let r = run_items_diff(&target, "Foo");
    assert_eq!(r.exit_code, Some(1), "expected drift exit; stderr={}", r.stderr);
    // helper should appear in in_source_only; render should be in both[].
    assert!(
        r.stdout.contains("helper"),
        "expected helper in in_source_only; stdout=\n{}",
        r.stdout
    );
}

#[test]
fn items_diff_signature_hint_divergence_flags_drift() {
    let target = tmp_dir("sig-drift");
    bootstrap_lib(&target);

    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render the frame."
satisfies = ["DEMO-1"]
signature_hint = "fn render(x: u64) -> u32"
"#,
    )
    .unwrap();

    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn render(x: u32) -> u32 { x }\n",
    )
    .unwrap();

    let r = run_items_diff(&target, "Foo");
    assert_eq!(r.exit_code, Some(1), "expected drift exit; stderr={}", r.stderr);
    assert!(
        r.stdout.contains("signature_drift"),
        "expected signature_drift; stdout=\n{}",
        r.stdout
    );
}

#[test]
fn items_diff_pre_implementation_module_marks_all_spec_only() {
    let target = tmp_dir("pre-impl");
    bootstrap_lib(&target);

    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render the frame."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    // No `src/foo.rs` — pre-implementation module.

    let r = run_items_diff(&target, "Foo");
    assert_eq!(r.exit_code, Some(1), "expected drift exit; stderr={}", r.stderr);
    assert!(r.stdout.contains("name: render"));
    assert!(r.stdout.contains("in_spec_only:"));
    // source_file is null when no source file resolves.
    assert!(r.stdout.contains("source_file: null"));
}

#[test]
fn items_diff_pairs_base_impl_spec_with_generic_source_impl() {
    // F-010: spec names an impl by its base type (`impl RateLimiter`, the
    // authoring convention), while the source impl is generic
    // (`impl<C> RateLimiter<C>`). The two must pair in both[] rather than
    // splitting into in_spec_only + in_source_only.
    let target = tmp_dir("generic-impl");
    bootstrap_lib(&target);

    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "RateLimiter"
kind = "struct"
description = "Token-bucket limiter."
satisfies = ["DEMO-1"]

[[items]]
name = "RateLimiter"
kind = "impl"
description = "Inherent methods on the limiter."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();

    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub struct RateLimiter<C> { _c: core::marker::PhantomData<C> }\n\
         impl<C> RateLimiter<C> { pub fn new() -> Self { Self { _c: core::marker::PhantomData } } }\n",
    )
    .unwrap();

    let r = run_items_diff(&target, "Foo");
    assert!(
        r.success,
        "expected exit 0 (generic impl paired); exit={:?} stdout=\n{}\nstderr={}",
        r.exit_code, r.stdout, r.stderr
    );
    assert!(
        r.stdout.contains("in_spec_only: []"),
        "impl should not fall into in_spec_only; stdout=\n{}",
        r.stdout
    );
    assert!(
        r.stdout.contains("in_source_only: []"),
        "generic impl should not fall into in_source_only; stdout=\n{}",
        r.stdout
    );
}

#[test]
fn items_diff_missing_module_errors_with_item_101() {
    let target = tmp_dir("missing");
    bootstrap_lib(&target);

    let r = run_items_diff(&target, "DoesNotExist");
    assert_eq!(r.exit_code, Some(2), "expected hard error; stderr={}", r.stderr);
    assert!(
        r.stderr.contains("ITEM-101"),
        "expected ITEM-101; stderr={}",
        r.stderr
    );
}
