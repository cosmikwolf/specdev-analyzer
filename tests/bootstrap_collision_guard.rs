//! `bootstrap` must not overwrite project files that are already there.
//!
//! BOOTSTRAP-001 tested exactly one directory, `specs/specdev/`. Everything
//! else the plan writes — `spec.toml` and every `specs/*.toml` — had no check,
//! so a tree carrying hand-authored requirements, decisions or architecture
//! lost them to the profile template, silently, at exit 0.
//!
//! Source and the build manifest were never at risk: bootstrap does not write
//! to `src/` or `Cargo.toml`. The last test here pins that, so the guard is
//! not mistaken for protecting more than it does.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::bootstrap::{self, BootstrapInputs, BootstrapOptions};

const SENTINEL: &str = "sentinel-99";

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-bootstrap-collision-{tag}-{}-{}",
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

fn inputs(target: &Path) -> BootstrapInputs {
    BootstrapInputs {
        profile: "library-api".into(),
        language: "rust".into(),
        project_name: "collision_demo".into(),
        target_dir: target.to_path_buf(),
    }
}

fn run(target: &Path) -> Result<(), String> {
    bootstrap::run_bootstrap(&full_plugin(), &inputs(target)).map_err(|e| format!("{e:#}"))
}

fn run_forced(target: &Path) -> Result<(), String> {
    bootstrap::run_bootstrap_with(
        &full_plugin(),
        &inputs(target),
        BootstrapOptions { force: true },
    )
    .map_err(|e| format!("{e:#}"))
}

/// Hand-authored spec files, of the kind a project accumulates before or
/// instead of running bootstrap.
fn seed_handwritten_specs(target: &Path) {
    fs::create_dir_all(target.join("specs")).unwrap();
    fs::write(
        target.join("spec.toml"),
        format!("[project]\nlicense = \"Proprietary-{SENTINEL}\"\n"),
    )
    .unwrap();
    fs::write(
        target.join("specs/architecture.toml"),
        format!("# HAND-AUTHORED {SENTINEL}\n[platform]\nname = \"precious\"\n"),
    )
    .unwrap();
}

#[test]
fn existing_spec_files_are_refused_and_left_byte_identical() {
    let target = tmp("refuse").join("proj");
    fs::create_dir_all(&target).unwrap();
    seed_handwritten_specs(&target);
    let before_spec = fs::read_to_string(target.join("spec.toml")).unwrap();
    let before_arch = fs::read_to_string(target.join("specs/architecture.toml")).unwrap();

    let err = run(&target).expect_err("must refuse");

    assert!(err.contains("[BOOTSTRAP-003]"), "{err}");
    assert!(
        err.contains("spec.toml"),
        "every collision is named in one message, the way INLINE-002 does it: {err}"
    );
    assert!(err.contains("--force"), "the way out is named: {err}");
    assert_eq!(
        fs::read_to_string(target.join("spec.toml")).unwrap(),
        before_spec
    );
    assert_eq!(
        fs::read_to_string(target.join("specs/architecture.toml")).unwrap(),
        before_arch
    );
    assert!(
        !target.join("specs/specdev").exists(),
        "a refusal must write nothing at all, not even the parts that would not collide"
    );
}

#[test]
fn force_overwrites_them() {
    let target = tmp("force").join("proj");
    fs::create_dir_all(&target).unwrap();
    seed_handwritten_specs(&target);

    run_forced(&target).expect("--force proceeds");

    assert!(
        !fs::read_to_string(target.join("spec.toml"))
            .unwrap()
            .contains(SENTINEL),
        "--force means overwrite"
    );
    assert!(target.join("specs/specdev").is_dir());
}

#[test]
fn a_greenfield_tree_is_untouched_by_the_guard() {
    let target = tmp("greenfield").join("proj");
    fs::create_dir_all(&target).unwrap();

    run(&target).expect("nothing to collide with");

    assert!(target.join("spec.toml").is_file());
    assert!(target.join("specs/specdev").is_dir());
}

#[test]
fn re_bootstrap_still_reports_bootstrap_001_not_003() {
    // BOOTSTRAP-001 is the more specific diagnosis and must keep winning:
    // "you already bootstrapped here" beats "these files exist".
    let target = tmp("rebootstrap").join("proj");
    fs::create_dir_all(&target).unwrap();
    run(&target).expect("first bootstrap");

    let err = run(&target).expect_err("second must refuse");

    assert!(err.contains("[BOOTSTRAP-001]"), "{err}");
    assert!(!err.contains("[BOOTSTRAP-003]"), "{err}");
}

#[test]
fn existing_source_and_manifest_do_not_block_adoption() {
    // The guard protects the spec tree, not the whole directory. A real
    // codebase adopting specdev has src/ and a manifest already, and must
    // still be able to bootstrap — that is the entire adoption story.
    let target = tmp("adoption").join("proj");
    fs::create_dir_all(target.join("src")).unwrap();
    fs::write(target.join("src/main.rs"), format!("// {SENTINEL}\n")).unwrap();
    fs::write(
        target.join("Cargo.toml"),
        format!("# {SENTINEL}\n[package]\n"),
    )
    .unwrap();

    run(&target).expect("source and manifest are not the spec tree");

    assert!(
        fs::read_to_string(target.join("src/main.rs"))
            .unwrap()
            .contains(SENTINEL),
        "bootstrap never writes into src/"
    );
    assert!(
        fs::read_to_string(target.join("Cargo.toml"))
            .unwrap()
            .contains(SENTINEL),
        "bootstrap never writes the build manifest — that is manifest --write's job, \
         and #148 guards it there"
    );
}
