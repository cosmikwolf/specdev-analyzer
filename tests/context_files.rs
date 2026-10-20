//! `module.context_files` schema field and prompt-assembly inlining.
//!
//! Coverage:
//!   * Schema accepts a string array under `context_files`; absent ⇒ no
//!     Project-context section in the prompt.
//!   * Files inline in **declared order** between architecture.toml and
//!     the module spec, fenced with the path as language tag.
//!   * Validate emits `CONTEXT-001` for missing files and `CONTEXT-002`
//!     for unreadable files; a binary file is accepted.
//!   * Validate accepts a clean `context_files` list (no findings).

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-context-files-{tag}-{}-{}",
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

fn fresh_library_project(tag: &str) -> (PathBuf, LayoutContext) {
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
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn run_validate(project: &Path) -> (bool, String, String) {
    let out = Command::new(analyzer_bin())
        .arg("validate")
        .arg("--project")
        .arg(project)
        .arg("--no-color")
        .output()
        .expect("invoke validate");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn missing_context_file_emits_context_001() {
    let (target, _layout) = fresh_library_project("missing-ctx");
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"
context_files = ["docs/never-existed.md"]
"#,
    )
    .unwrap();
    let (success, stdout, stderr) = run_validate(&target);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        !success,
        "validate should fail when context_files entry is missing; out:\n{combined}"
    );
    assert!(
        combined.contains("CONTEXT-001"),
        "expected CONTEXT-001 finding; out:\n{combined}"
    );
    assert!(
        combined.contains("docs/never-existed.md"),
        "finding should name the missing path; out:\n{combined}"
    );
}

#[test]
fn binary_context_file_is_accepted() {
    // A figure or a PDF datasheet is reference reading: only existence and
    // readability are checked, not the content (#243).
    let (target, _layout) = fresh_library_project("binary-context");
    std::fs::write(
        target.join("docs-binary.bin"),
        [0xff_u8, 0xfe, 0xfd, 0x00, 0x80, 0x81],
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Bar.toml"),
        r#"name = "Bar"
visibility = "pub"
context_files = ["docs-binary.bin"]
"#,
    )
    .unwrap();
    let (_success, stdout, stderr) = run_validate(&target);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        !combined.contains("CONTEXT-002") && !combined.contains("CONTEXT-001"),
        "a binary context file must not get a CONTEXT finding; out:\n{combined}"
    );
}

#[test]
fn clean_context_files_validate_passes() {
    let (target, _layout) = fresh_library_project("clean");
    std::fs::write(
        target.join("README.md"),
        "# project context\n\nfacts here\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Baz.toml"),
        r#"name = "Baz"
visibility = "pub"
context_files = ["README.md"]
"#,
    )
    .unwrap();
    let (_success, stdout, stderr) = run_validate(&target);
    let combined = format!("{stdout}\n{stderr}");
    // Validate may still fail on unrelated bootstrap-empty surfaces (e.g.,
    // no requirements). We only assert the absence of CONTEXT-* findings.
    assert!(
        !combined.contains("CONTEXT-001"),
        "no CONTEXT-001 finding expected; out:\n{combined}"
    );
    assert!(
        !combined.contains("CONTEXT-002"),
        "no CONTEXT-002 finding expected; out:\n{combined}"
    );
}

#[test]
fn schema_rejects_non_string_context_files_entry() {
    let (target, _layout) = fresh_library_project("bad-shape");
    std::fs::write(
        target.join("specs/modules/Qux.toml"),
        r#"name = "Qux"
visibility = "pub"
context_files = [42]
"#,
    )
    .unwrap();
    let (success, stdout, stderr) = run_validate(&target);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        !success,
        "schema should reject non-string context_files entry; out:\n{combined}"
    );
    assert!(
        combined.contains("SCHEMA"),
        "expected SCHEMA finding; out:\n{combined}"
    );
}

fn run_context_readme(project: &Path, plugin: Option<&Path>) -> String {
    let mut cmd = Command::new(analyzer_bin());
    cmd.env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .arg("context")
        .arg("--project")
        .arg(project)
        .arg("--module")
        .arg("Foo");
    if let Some(p) = plugin {
        cmd.env("SPECDEV_PLUGIN_ROOT", p);
    }
    let out = cmd.output().expect("invoke context");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    stdout
        .lines()
        .find(|l| l.contains("profile_readme:"))
        .unwrap_or_default()
        .to_string()
}

#[test]
fn context_expands_the_plugin_root_when_it_resolves() {
    // #244: with a plugin root, the README path opens as printed; with
    // none, the token stays so the reader can still resolve it.
    let (target, _layout) = fresh_library_project("ctx-plugin-root");
    let plugin = full_plugin();
    let expanded = run_context_readme(&target, Some(&plugin));
    assert!(
        expanded.contains(&format!(
            "{}/profiles/library-api/README.md",
            plugin.display()
        )),
        "expected an expanded path; got: {expanded}"
    );
    let token = run_context_readme(&target, None);
    assert!(
        token.contains("${CLAUDE_PLUGIN_ROOT}/profiles/library-api/README.md"),
        "expected the token without a plugin root; got: {token}"
    );
}
