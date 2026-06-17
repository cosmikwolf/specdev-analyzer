//! `module.context_files` schema field and prompt-assembly inlining.
//!
//! Coverage:
//!   * Schema accepts a string array under `context_files`; absent ⇒ no
//!     Project-context section in the prompt.
//!   * Files inline in **declared order** between architecture.toml and
//!     the module spec, fenced with the path as language tag.
//!   * Validate emits `CONTEXT-001` for missing files and `CONTEXT-002`
//!     for non-UTF-8 / unreadable files.
//!   * Validate accepts a clean `context_files` list (no findings).

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    prompt::assemble_module_prompt,
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
fn module_without_context_files_omits_project_context_section() {
    let (target, layout) = fresh_library_project("no-context");
    std::fs::write(
        target.join("specs/modules/Plain.toml"),
        r#"name = "Plain"
visibility = "pub"
"#,
    )
    .unwrap();
    let prompt = assemble_module_prompt(&layout, "Plain").unwrap();
    assert!(
        !prompt.contains("## Project context"),
        "no context_files declared -> no Project context section; got:\n{prompt}"
    );
}

#[test]
fn context_files_inline_in_declared_order_between_architecture_and_module() {
    let (target, layout) = fresh_library_project("declared-order");
    // Two distinguishable context files, declared in non-alphabetical order
    // so we can verify declared-order is honored over lex order.
    std::fs::write(
        target.join("docs-zeta.md"),
        "ZETA-CONTENT first inlined\n",
    )
    .unwrap();
    std::fs::write(
        target.join("docs-alpha.md"),
        "ALPHA-CONTENT second inlined\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Envelope.toml"),
        r#"name = "Envelope"
visibility = "pub"
context_files = [
  "docs-zeta.md",
  "docs-alpha.md",
]
"#,
    )
    .unwrap();
    let prompt = assemble_module_prompt(&layout, "Envelope").unwrap();
    assert!(prompt.contains("## Project context"), "section header expected");
    assert!(prompt.contains("```docs-zeta.md\n"), "fence with path-as-tag");
    assert!(prompt.contains("ZETA-CONTENT first inlined"));
    assert!(prompt.contains("ALPHA-CONTENT second inlined"));

    // Declared order: zeta before alpha; architecture before context;
    // context before module spec.
    let arch_idx = prompt.find("## Architecture").expect("arch section");
    let ctx_idx = prompt.find("## Project context").expect("ctx section");
    let module_idx = prompt.find("## Module spec").expect("module section");
    let zeta_idx = prompt.find("ZETA-CONTENT").expect("zeta inlined");
    let alpha_idx = prompt.find("ALPHA-CONTENT").expect("alpha inlined");
    assert!(arch_idx < ctx_idx, "Architecture must come before context");
    assert!(ctx_idx < module_idx, "Context must come before Module spec");
    assert!(zeta_idx < alpha_idx, "Declared order: zeta before alpha");
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
fn non_utf8_context_file_emits_context_002() {
    let (target, _layout) = fresh_library_project("non-utf8");
    // Non-UTF-8 byte sequence
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
    let (success, stdout, stderr) = run_validate(&target);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        !success,
        "validate should fail when context file is non-UTF-8; out:\n{combined}"
    );
    assert!(
        combined.contains("CONTEXT-002"),
        "expected CONTEXT-002 finding; out:\n{combined}"
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
