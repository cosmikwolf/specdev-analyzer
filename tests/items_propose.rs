//! `items propose` Stage-2 scaffolding.
//!
//! `items propose` is purely deterministic: it wraps `items diff` output
//! in a structured proposal envelope, enriches each entry with source
//! snippets and spec mappings, and writes the file to
//! `specs/specdev/upstream/items-sync/<module>.proposal.yaml`. No LLM
//! is involved at the analyzer layer; that's the skill's job.
//!
//! Coverage:
//!   * Proposal file lands at the conventional path with the expected
//!     status / entries when drift is present.
//!   * No drift → status: no-drift, entries: [], file still written.
//!   * `--clobber` is required to overwrite an existing proposal.
//!   * In-source-only entries carry the source snippet sliced by span.
//!   * In-spec-only entries carry the spec_item mapping and no snippet.
//!   * Pre-implementation modules emit only in_spec_only entries.
//!   * Exit code: 0 when clean, 1 when drift, 2 on hard error.

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
        "specdev-items-propose-{tag}-{}-{}",
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
    stderr: String,
}

fn run_propose(project: &Path, module: &str, extra: &[&str]) -> CliResult {
    let mut cmd = Command::new(analyzer_bin());
    cmd.arg("items")
        .arg("propose")
        .arg("--project")
        .arg(project)
        .arg("--module")
        .arg(module);
    for a in extra {
        cmd.arg(a);
    }
    let out = cmd.output().expect("invoke items propose");
    CliResult {
        success: out.status.success(),
        exit_code: out.status.code(),
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

fn proposal_path(target: &Path, module: &str) -> PathBuf {
    target
        .join("specs/specdev/upstream/items-sync")
        .join(format!("{module}.proposal.yaml"))
}

#[test]
fn propose_writes_file_at_conventional_path() {
    let target = tmp_dir("path");
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

    let r = run_propose(&target, "Foo", &[]);
    assert_eq!(
        r.exit_code,
        Some(1),
        "expected drift exit 1; stderr={}",
        r.stderr
    );
    let path = proposal_path(&target, "Foo");
    assert!(
        path.exists(),
        "proposal file not written at {}",
        path.display()
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("schema_version: 1"));
    assert!(content.contains("module: Foo"));
    assert!(content.contains("status: needs-rationale"));
    assert!(content.contains("approved: false"));
    assert!(content.contains("source-only-0"));
    assert!(content.contains("name: helper"));
    // Snippet text from the source file.
    assert!(content.contains("pub fn helper() -> u32"));
}

#[test]
fn propose_no_drift_writes_no_drift_status_and_exits_zero() {
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
    std::fs::write(target.join("src/foo.rs"), "pub fn render() -> u32 { 0 }\n").unwrap();

    let r = run_propose(&target, "Foo", &[]);
    assert!(r.success, "expected exit 0 (clean); stderr={}", r.stderr);
    let content = std::fs::read_to_string(proposal_path(&target, "Foo")).unwrap();
    assert!(content.contains("status: no-drift"));
    assert!(content.contains("entries: []"));
}

#[test]
fn propose_refuses_to_clobber_without_flag() {
    let target = tmp_dir("clobber");
    bootstrap_lib(&target);
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "ghost"
kind = "fn"
description = "Spec-only fn."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();

    let first = run_propose(&target, "Foo", &[]);
    assert_eq!(
        first.exit_code,
        Some(1),
        "first run drift: stderr={}",
        first.stderr
    );
    let path = proposal_path(&target, "Foo");
    // Mark it with a fake rationale so we can detect overwrite.
    let original = std::fs::read_to_string(&path).unwrap();
    let edited = original.replace("rationale: ''", "rationale: 'human review'");
    std::fs::write(&path, &edited).unwrap();

    let second = run_propose(&target, "Foo", &[]);
    assert_eq!(
        second.exit_code,
        Some(1),
        "expected refusal exit 1; stderr={}",
        second.stderr
    );
    assert!(
        second.stderr.contains("ITEM-202"),
        "expected ITEM-202; stderr={}",
        second.stderr
    );
    let after = std::fs::read_to_string(&path).unwrap();
    assert_eq!(after, edited, "file should not have been overwritten");

    let third = run_propose(&target, "Foo", &["--clobber"]);
    assert_eq!(
        third.exit_code,
        Some(1),
        "clobber drift exit; stderr={}",
        third.stderr
    );
    let after_clobber = std::fs::read_to_string(&path).unwrap();
    assert!(
        !after_clobber.contains("human review"),
        "expected --clobber to drop the rationale; got\n{after_clobber}"
    );
}

#[test]
fn propose_in_spec_only_carries_spec_item_mapping() {
    let target = tmp_dir("spec-only");
    bootstrap_lib(&target);
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "ghost"
kind = "fn"
description = "Spec-only fn awaiting implementation."
satisfies = ["DEMO-1", "DEMO-2"]
"#,
    )
    .unwrap();
    // Write empty source file so resolution succeeds with no items.
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(target.join("src/foo.rs"), "// empty\n").unwrap();

    let r = run_propose(&target, "Foo", &[]);
    assert_eq!(r.exit_code, Some(1));
    let content = std::fs::read_to_string(proposal_path(&target, "Foo")).unwrap();
    assert!(content.contains("category: in_spec_only"));
    assert!(content.contains("name: ghost"));
    // spec_item nested mapping carries the original fields.
    assert!(content.contains("spec_item:"));
    assert!(content.contains("description: Spec-only fn awaiting implementation."));
    assert!(content.contains("- DEMO-1"));
    assert!(content.contains("- DEMO-2"));
    // No source snippet for in_spec_only.
    assert!(!content.contains("source_snippet"));
}

#[test]
fn propose_pre_implementation_module_marks_all_spec_only() {
    let target = tmp_dir("pre-impl");
    bootstrap_lib(&target);
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "ghost"
kind = "fn"
description = "Spec-only."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    // No src/foo.rs.

    let r = run_propose(&target, "Foo", &[]);
    assert_eq!(r.exit_code, Some(1));
    let content = std::fs::read_to_string(proposal_path(&target, "Foo")).unwrap();
    assert!(
        !content.contains("source_file:"),
        "no source_file field when None: {content}"
    );
    assert!(content.contains("category: in_spec_only"));
}

#[test]
fn propose_output_override_writes_to_explicit_path() {
    let target = tmp_dir("output");
    bootstrap_lib(&target);
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "ghost"
kind = "fn"
description = "x."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();

    let custom = target.join("scratch/proposal.yaml");
    let r = run_propose(&target, "Foo", &["--output", custom.to_str().unwrap()]);
    assert_eq!(r.exit_code, Some(1));
    assert!(
        custom.exists(),
        "custom path not written: {}",
        custom.display()
    );
    // Default path should NOT have been written when --output overrides.
    assert!(!proposal_path(&target, "Foo").exists());
}

#[test]
fn propose_missing_module_errors_with_item_101() {
    let target = tmp_dir("missing");
    bootstrap_lib(&target);

    let r = run_propose(&target, "DoesNotExist", &[]);
    assert_eq!(
        r.exit_code,
        Some(2),
        "expected hard error; stderr={}",
        r.stderr
    );
    assert!(
        r.stderr.contains("ITEM-101"),
        "expected ITEM-101; stderr={}",
        r.stderr
    );
}
