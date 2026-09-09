//! Profile-declared `paired_files` — written once when absent by
//! `manifest --write`, language-neutral, gated on `--write`, one summary
//! line for kept files, two-writers refusal.
//!
//! The `nextjs-app` fixture profile declares six rows (`tsconfig.json`,
//! `next.config.ts`, `vitest.config.ts`, `playwright.config.ts`,
//! `.gitignore`, `app/.gitkeep`). These tests run the real binary so the
//! shared write loop in `run_manifest` is what is exercised.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};
use specdev_analyzer::manifest::Profile;

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-paired-files-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

fn fresh_nextjs_app(tag: &str) -> PathBuf {
    let target = tmp_workspace(tag).join("proj");
    fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "nextjs-app".into(),
            language: "typescript".into(),
            project_name: "hello_next".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

fn manifest(project: &Path, args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .arg("manifest")
        .arg("--project")
        .arg(project)
        .args(args)
        .output()
        .expect("spawn specdev-analyzer");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

const PAIRED: &[&str] = &[
    "tsconfig.json",
    "next.config.ts",
    "vitest.config.ts",
    "playwright.config.ts",
    ".gitignore",
    "app/.gitkeep",
];

#[test]
fn nextjs_app_profile_declares_six_paired_files_and_bootstrap_copies_templates() {
    let project = fresh_nextjs_app("decl");
    let profile = Profile::load(&project.join("specs/specdev/profile.yaml")).unwrap();
    assert_eq!(profile.paired_files.len(), 6);
    for row in &profile.paired_files {
        let t = project.join("specs/specdev").join(&row.template_file);
        assert!(t.is_file(), "bootstrap must copy {}", t.display());
    }
}

#[test]
fn write_creates_paired_files_once_and_keeps_them_after() {
    let project = fresh_nextjs_app("write");
    for rel in PAIRED {
        assert!(!project.join(rel).exists(), "{rel} must not pre-exist");
    }
    let (code, err) = manifest(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(project.join("package.json").is_file());
    for rel in PAIRED {
        assert!(
            project.join(rel).is_file(),
            "{rel} missing after --write\n{err}"
        );
        assert!(err.contains("wrote paired file"), "{err}");
    }
    assert!(
        !err.contains("kept "),
        "first write must keep nothing:\n{err}"
    );
    // tsconfig.json is the post-`next build` shape with strict on.
    let ts = fs::read_to_string(project.join("tsconfig.json")).unwrap();
    assert!(ts.contains("\"strict\": true"));
    assert!(ts.contains(".next/types/**/*.ts"));

    // Second write: every file exists → one summary line, no rewrite.
    fs::write(project.join("next.config.ts"), "// user-owned\n").unwrap();
    let (code, err) = manifest(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("kept 6 existing paired file(s):"),
        "expected one summary line, got:\n{err}"
    );
    assert_eq!(
        err.matches("kept ").count(),
        1,
        "exactly one kept line, not one per file:\n{err}"
    );
    assert_eq!(
        fs::read_to_string(project.join("next.config.ts")).unwrap(),
        "// user-owned\n",
        "an existing paired file is never overwritten"
    );

    // A deleted paired file comes back — "written when absent".
    fs::remove_file(project.join("app/.gitkeep")).unwrap();
    let (code, err) = manifest(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(project.join("app/.gitkeep").is_file());
    assert!(err.contains("kept 5 existing paired file(s):"), "{err}");
}

#[test]
fn output_redirect_writes_no_paired_file() {
    let project = fresh_nextjs_app("output");
    let out_file = project.parent().unwrap().join("pkg.json");
    let (code, err) = manifest(&project, &["--output", out_file.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    assert!(out_file.is_file());
    for rel in PAIRED {
        assert!(
            !project.join(rel).exists(),
            "{rel} must not be written under --output\n{err}"
        );
    }
    assert!(
        err.contains("paired file(s) not written under --output"),
        "{err}"
    );
}

fn rewrite_profile(project: &Path, extra_row: &str) {
    let p = project.join("specs/specdev/profile.yaml");
    let body = fs::read_to_string(&p).unwrap();
    let body = body.replace(
        "paired_files:\n",
        &format!("paired_files:\n  - {extra_row}\n"),
    );
    fs::write(&p, body).unwrap();
}

#[test]
fn paired_target_equal_to_the_manifest_file_is_refused() {
    let project = fresh_nextjs_app("collide");
    rewrite_profile(
        &project,
        "{ template_file: templates/gitkeep.template, target: package.json }",
    );
    let (code, err) = manifest(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("two writers"), "{err}");
    assert!(err.contains("package.json"), "{err}");
    assert!(
        !project.join("tsconfig.json").exists(),
        "nothing is written when the set is refused"
    );
}

#[test]
fn paired_target_escaping_the_project_root_is_refused() {
    let project = fresh_nextjs_app("escape");
    rewrite_profile(
        &project,
        "{ template_file: templates/gitkeep.template, target: ../outside.txt }",
    );
    let (code, err) = manifest(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("relative path inside"), "{err}");
    assert!(!project.parent().unwrap().join("outside.txt").exists());
}

#[test]
fn missing_template_is_an_error_naming_the_path() {
    let project = fresh_nextjs_app("missing");
    rewrite_profile(
        &project,
        "{ template_file: templates/nope.template, target: nope.txt }",
    );
    let (code, err) = manifest(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("nope.template"), "{err}");
}

#[test]
fn profile_without_paired_files_still_loads() {
    let p = full_plugin().join("profiles/flutter-app/profile.yaml");
    let profile = Profile::load(&p).unwrap();
    assert!(profile.paired_files.is_empty());
}
