//! `manifest --write` must not replace a build manifest it did not write.
//!
//! The manifest is pure output and is rewritten on every `--write`. That is
//! correct for a project specdev generated, and wrong for a project that
//! adopted specdev over a tree it already had: there the file on disk is
//! hand-authored, and the write discarded it silently at exit 0.
//!
//! The guard is the one `derive` already applies to its own targets — is
//! there a row for this path in `gen-lineage.toml`? These tests pin all four
//! corners of that matrix, because three of them are paths that must keep
//! working and only one is the bug:
//!
//! | on disk | vs render | recorded | outcome            |
//! |---------|-----------|----------|--------------------|
//! | absent  | —         | —        | write (greenfield) |
//! | present | identical | —        | write (nothing to lose) |
//! | present | differs   | yes      | write (regen loop) |
//! | present | differs   | no       | REFUSE   <-- #148  |
//! | present | differs   | no       | write, with --accept-edits |
//!
//! The identical-bytes row is tested BEFORE the lineage row, exactly as
//! `derive`'s matrix does it. Getting that order wrong blocks a repeat
//! `manifest --write` that has nothing to record — caught by
//! `manifest_package_json.rs`'s pnpm-workspace test, not by this file.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

const SENTINEL: &str = "sentinel-12345";

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-manifest-unrecorded-{tag}-{}-{}",
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

fn fresh(tag: &str) -> PathBuf {
    let project = tmp_workspace(tag).join("proj");
    fs::create_dir_all(&project).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "brownfield_demo".into(),
            target_dir: project.clone(),
        },
    )
    .unwrap();
    project
}

fn run(project: &Path, sub: &str, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .arg(sub)
        .arg("--project")
        .arg(project)
        .args(args)
        .env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .output()
        .expect("spawn specdev-analyzer");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A hand-authored manifest, carrying fields the spec has no place for.
fn write_handwritten_manifest(project: &Path) -> String {
    let body = format!(
        "[package]\n\
         name = \"brownfield_demo\"\n\
         version = \"9.9.9-handwritten\"\n\
         edition = \"2021\"\n\
         authors = [\"{SENTINEL}\"]\n\
         \n\
         [package.metadata.sentinel]\n\
         marker = \"{SENTINEL}\"\n\
         \n\
         [dependencies]\n\
         anyhow = \"1.0\"\n"
    );
    fs::write(project.join("Cargo.toml"), &body).unwrap();
    body
}

fn record_manifest(project: &Path) {
    let abs = project.join("Cargo.toml");
    let (code, _, err) = run(
        project,
        "gen-record",
        &["--accept-edits", abs.to_str().unwrap()],
    );
    assert_eq!(code, 0, "gen-record should succeed: {err}");
}

#[test]
fn unrecorded_manifest_at_target_is_refused_and_left_intact() {
    let project = fresh("refuse");
    let before = write_handwritten_manifest(&project);

    let (code, _, err) = run(&project, "manifest", &["--write"]);

    assert_eq!(
        code, 1,
        "refusal is a data-level refusal, not a crash: {err}"
    );
    assert!(
        err.contains("[MANIFEST-003]"),
        "refusal carries its code: {err}"
    );
    assert!(
        err.contains("Cargo.toml"),
        "refusal names the file it protected: {err}"
    );
    assert!(
        err.contains("--accept-edits"),
        "refusal names the way out: {err}"
    );
    assert_eq!(
        fs::read_to_string(project.join("Cargo.toml")).unwrap(),
        before,
        "the hand-authored manifest must be byte-identical after a refusal"
    );
}

#[test]
fn accept_edits_overwrites_an_unrecorded_manifest() {
    let project = fresh("accept");
    let before = write_handwritten_manifest(&project);

    let (code, _, err) = run(&project, "manifest", &["--write", "--accept-edits"]);

    assert_eq!(code, 0, "{err}");
    let after = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert_ne!(after, before, "--accept-edits means overwrite");
    assert!(
        !after.contains(SENTINEL),
        "the spec-rendered manifest replaces the hand-authored one"
    );
}

#[test]
fn greenfield_write_is_untouched_by_the_guard() {
    let project = fresh("greenfield");
    assert!(
        !project.join("Cargo.toml").exists(),
        "bootstrap must not leave a manifest behind, or this test proves nothing"
    );

    let (code, _, err) = run(&project, "manifest", &["--write"]);

    assert_eq!(
        code, 0,
        "nothing at the target means nothing to protect: {err}"
    );
    assert!(project.join("Cargo.toml").exists());
}

#[test]
fn a_recorded_manifest_still_rewrites_on_every_run() {
    // The regenerate loop is the path that must NOT regress: /spec-gen
    // writes the manifest and records it, then rewrites it on every later
    // run. A guard that blocked this would break normal codegen.
    let project = fresh("regen");
    let (code, _, err) = run(&project, "manifest", &["--write"]);
    assert_eq!(code, 0, "{err}");
    record_manifest(&project);

    for run_no in 1..=3 {
        let (code, _, err) = run(&project, "manifest", &["--write"]);
        assert_eq!(code, 0, "recorded manifest, run {run_no}: {err}");
    }
}

#[test]
fn output_flag_is_the_callers_business_and_is_not_guarded() {
    // `--output <path>` names a destination explicitly, the same class as a
    // shell redirection. The guard is scoped to `--write` on purpose.
    let project = fresh("output");
    write_handwritten_manifest(&project);
    let dest = project.join("elsewhere.toml");

    let (code, _, err) = run(&project, "manifest", &["--output", dest.to_str().unwrap()]);

    assert_eq!(code, 0, "{err}");
    assert!(dest.exists(), "--output wrote where it was told");
    assert!(
        fs::read_to_string(project.join("Cargo.toml"))
            .unwrap()
            .contains(SENTINEL),
        "--output must not touch the project-root manifest"
    );
}

#[test]
fn a_repeat_write_with_nothing_recorded_is_not_a_conflict() {
    // The analyzer is also a standalone CLI, and /spec-gen can be
    // interrupted between Step 6 (write) and Step 6.5 (record). Neither
    // leaves a file the user authored, so neither is a conflict: the bytes
    // on disk are already ours, byte for byte.
    let project = fresh("repeat");
    let (code, _, err) = run(&project, "manifest", &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        !project.join("specs/specdev/gen-lineage.toml").exists(),
        "nothing recorded — that is the point of this test"
    );

    let (code, _, err) = run(&project, "manifest", &["--write"]);
    assert_eq!(
        code, 0,
        "identical bytes are not a conflict, recorded or not: {err}"
    );
}

#[test]
fn only_bytes_that_differ_are_protected() {
    // Pins the two halves of the rule against each other in one test, so a
    // future change cannot satisfy one and quietly drop the other.
    let project = fresh("halves");
    let (code, _, err) = run(&project, "manifest", &["--write"]);
    assert_eq!(code, 0, "{err}");
    let rendered = fs::read_to_string(project.join("Cargo.toml")).unwrap();

    // Same bytes, unrecorded -> allowed.
    let (code, _, _) = run(&project, "manifest", &["--write"]);
    assert_eq!(code, 0, "identical + unrecorded must write");

    // Different bytes, unrecorded -> refused.
    fs::write(
        project.join("Cargo.toml"),
        format!("{rendered}\n[package.metadata.mine]\nmarker = \"{SENTINEL}\"\n"),
    )
    .unwrap();
    let (code, _, err) = run(&project, "manifest", &["--write"]);
    assert_eq!(code, 1, "different + unrecorded must refuse: {err}");
    assert!(
        fs::read_to_string(project.join("Cargo.toml"))
            .unwrap()
            .contains(SENTINEL),
        "and must leave the bytes alone"
    );
}
