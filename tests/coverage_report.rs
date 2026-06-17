//! Integration tests for the `report coverage` subcommand.
//!
//! Coverage:
//!   * Empty `requirements.toml` → empty matrix, no error.
//!   * Orphan requirement → row present with no satisfiers, also listed
//!     under `orphans`.
//!   * Multi-satisfier requirement → row lists every satisfying module.
//!   * Item-level `satisfies` counts as module satisfaction.
//!   * Markdown rendering opt-in via `--format markdown`.
//!   * Default format is YAML when `--format` is omitted.
//!   * Unknown `--format` value errors with `COVERAGE-001`.

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
        "specdev-coverage-{tag}-{}-{}",
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

fn fresh_library_project(tag: &str) -> PathBuf {
    let target = tmp_dir(tag);
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
    target
}

fn run_coverage(project: &Path, format: Option<&str>) -> (bool, String, String) {
    let mut cmd = Command::new(analyzer_bin());
    cmd.arg("report").arg("coverage").arg("--project").arg(project);
    if let Some(f) = format {
        cmd.arg("--format").arg(f);
    }
    let out = cmd.output().expect("invoke report coverage");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn empty_requirements_emits_empty_matrix_no_error() {
    let target = fresh_library_project("empty");
    // Bootstrap leaves requirements.toml empty by default; no need to overwrite.
    let (success, stdout, stderr) = run_coverage(&target, None);
    assert!(
        success,
        "report coverage must succeed on empty requirements; stderr:\n{stderr}"
    );
    assert!(
        stdout.contains("matrix:") || stdout.contains("matrix: []"),
        "YAML output expected with `matrix:` key; got:\n{stdout}"
    );
    assert!(
        stdout.contains("orphans:") || stdout.contains("orphans: []"),
        "YAML output expected with `orphans:` key; got:\n{stdout}"
    );
}

#[test]
fn orphan_requirement_flagged_in_matrix_and_orphans() {
    let target = fresh_library_project("orphan");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-001"
nature = "functional"
text = "the orphan requirement"
"#,
    )
    .unwrap();
    let (success, stdout, _stderr) = run_coverage(&target, None);
    assert!(success);
    // The matrix carries a row for REQ-001; satisfied_by is empty; the
    // requirement also appears in the orphans list.
    assert!(stdout.contains("REQ-001"), "matrix should mention REQ-001");
    let orphans_idx = stdout.find("orphans:").expect("orphans key present");
    assert!(
        stdout[orphans_idx..].contains("REQ-001"),
        "REQ-001 should appear under orphans:; got:\n{stdout}"
    );
}

#[test]
fn multi_satisfier_requirement_lists_all_modules() {
    let target = fresh_library_project("multi");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-002"
nature = "functional"
text = "satisfied by two modules"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Alpha.toml"),
        r#"name = "Alpha"
visibility = "pub"
satisfies = ["REQ-002"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Beta.toml"),
        r#"name = "Beta"
visibility = "pub"
satisfies = ["REQ-002"]
"#,
    )
    .unwrap();
    let (success, stdout, _stderr) = run_coverage(&target, None);
    assert!(success);
    // Both module names should appear under REQ-002's satisfied_by row.
    let req_idx = stdout.find("REQ-002").expect("REQ-002 row present");
    let tail = &stdout[req_idx..];
    assert!(tail.contains("Alpha"), "Alpha should satisfy REQ-002; got:\n{stdout}");
    assert!(tail.contains("Beta"), "Beta should satisfy REQ-002; got:\n{stdout}");
}

#[test]
fn item_level_satisfies_counts_as_module_satisfaction() {
    let target = fresh_library_project("item-level");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-003"
nature = "functional"
text = "satisfied by an item, not the module"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Gamma.toml"),
        r#"name = "Gamma"
visibility = "pub"

[[items]]
kind = "fn"
name = "do_it"
description = "the satisfying item"
satisfies = ["REQ-003"]
"#,
    )
    .unwrap();
    let (success, stdout, _stderr) = run_coverage(&target, None);
    assert!(success);
    // REQ-003 is satisfied through an item-level satisfies, not the
    // module-level satisfies; coverage must still credit the module.
    let req_idx = stdout.find("REQ-003").expect("REQ-003 row present");
    let tail = &stdout[req_idx..];
    assert!(tail.contains("Gamma"), "Gamma should satisfy REQ-003 via item; got:\n{stdout}");
    // Should NOT be in orphans.
    let orphans_idx = stdout.find("orphans:").expect("orphans present");
    assert!(
        !stdout[orphans_idx..].contains("REQ-003"),
        "REQ-003 has a satisfier; it must not appear in orphans; got:\n{stdout}"
    );
}

#[test]
fn markdown_format_emits_table() {
    let target = fresh_library_project("md");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-010"
nature = "functional"
text = "covered"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Delta.toml"),
        r#"name = "Delta"
visibility = "pub"
satisfies = ["REQ-010"]
"#,
    )
    .unwrap();
    let (success, stdout, _stderr) = run_coverage(&target, Some("markdown")).clone();
    assert!(success);
    assert!(stdout.contains("# Coverage matrix"), "markdown header expected");
    assert!(stdout.contains("| Requirement | Satisfied by |"), "table header expected");
    assert!(stdout.contains("`REQ-010`"));
    assert!(stdout.contains("`Delta`"));
}

#[test]
fn markdown_format_flags_orphan_in_table() {
    let target = fresh_library_project("md-orphan");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-011"
nature = "functional"
text = "no satisfier"
"#,
    )
    .unwrap();
    let (success, stdout, _stderr) = run_coverage(&target, Some("md"));
    assert!(success);
    assert!(stdout.contains("ORPHAN"), "markdown ORPHAN highlight expected");
    assert!(stdout.contains("REQ-011"));
}

#[test]
fn default_format_is_yaml() {
    let target = fresh_library_project("default-yaml");
    let (success, stdout, _stderr) = run_coverage(&target, None);
    assert!(success);
    // YAML output uses `matrix:` (with a colon), not Markdown's `# Coverage matrix`.
    assert!(stdout.contains("matrix:"), "default format should be YAML; got:\n{stdout}");
    assert!(
        !stdout.contains("# Coverage matrix"),
        "default format must not be Markdown; got:\n{stdout}"
    );
}

#[test]
fn unknown_format_errors_coverage_001() {
    let target = fresh_library_project("bad-format");
    let (success, _stdout, stderr) = run_coverage(&target, Some("html"));
    assert!(!success, "unknown --format must fail");
    assert!(
        stderr.contains("COVERAGE-001"),
        "expected COVERAGE-001 in stderr; got:\n{stderr}"
    );
}

#[test]
fn declaration_order_preserved_in_matrix() {
    let target = fresh_library_project("order");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-Z"
nature = "functional"
text = "z first"

[[requirement]]
id = "REQ-A"
nature = "functional"
text = "a second"
"#,
    )
    .unwrap();
    let (success, stdout, _stderr) = run_coverage(&target, None);
    assert!(success);
    let z_idx = stdout.find("REQ-Z").expect("REQ-Z present");
    let a_idx = stdout.find("REQ-A").expect("REQ-A present");
    assert!(
        z_idx < a_idx,
        "Declaration order: REQ-Z should appear before REQ-A; got:\n{stdout}"
    );
}
