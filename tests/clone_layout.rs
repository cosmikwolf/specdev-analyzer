//! A project as git stores it: no empty directories.
//!
//! `bootstrap` creates the unit directories (`schemas/inlined/blocks/`,
//! `reductions/bridges/local/`, …) and `specs/modules/` empty. Git does not
//! store an empty directory, so every clone of a freshly bootstrapped
//! project — CI's checkout included — lacks them. An absent unit directory
//! means "nothing inlined yet", not a broken layout: `check` on a clone
//! must not warn about it. A missing required file still warns.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-clone-layout-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn analyzer(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(args)
        .arg("--no-color")
        .current_dir(dir)
        .env("SPECDEV_PLUGIN_ROOT", crate::ci::plugin_root())
        .output()
        .expect("run specdev-analyzer")
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Bootstrap a library-api project, commit it, and return a fresh clone.
fn bootstrapped_clone(tag: &str) -> PathBuf {
    let base = tmp(tag);
    let out = analyzer(
        &base,
        &["bootstrap", "--profile", "library-api", "--language", "rust", "--name", "clone_fixture", "proj"],
    );
    assert!(out.status.success(), "bootstrap failed: {}", String::from_utf8_lossy(&out.stderr));
    let proj = base.join("proj");
    git(&proj, &["init", "-q", "-b", "main"]);
    git(&proj, &["add", "-A"]);
    git(&proj, &["commit", "-q", "-m", "bootstrap"]);
    git(&base, &["clone", "-q", "proj", "clone"]);
    let clone = base.join("clone");
    assert!(
        !clone.join("specs/specdev/reductions/bridges/local").exists(),
        "precondition: the clone lacks the empty directories"
    );
    clone
}

fn layout_warnings(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&out.stderr).lines())
        .filter(|l| l.contains("[LAYOUT-003]"))
        .map(str::to_string)
        .collect()
}

#[test]
fn a_clone_of_a_bootstrapped_project_checks_without_layout_warnings() {
    let clone = bootstrapped_clone("clean");
    let out = analyzer(&clone, &["check"]);
    let all = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(all.contains("error(s)"), "check did not run:\n{all}");
    assert_eq!(layout_warnings(&out), Vec::<String>::new(), "\n{all}");
}

#[test]
fn a_clone_still_warns_about_a_missing_required_file() {
    let clone = bootstrapped_clone("required");
    std::fs::remove_file(clone.join("specs/specdev/reductions/project.yaml")).unwrap();
    let out = analyzer(&clone, &["validate"]);
    let warnings = layout_warnings(&out);
    assert!(
        warnings.iter().any(|w| w.contains("specs/specdev/reductions/project.yaml")),
        "expected LAYOUT-003 for the missing reduction; got {warnings:?}"
    );
}

#[test]
fn a_block_inlines_into_a_clone() {
    let clone = bootstrapped_clone("inline");
    let out = analyzer(&clone, &["inline", "embassy"]);
    assert!(out.status.success(), "inline into a clone failed: {}", String::from_utf8_lossy(&out.stderr));
    assert!(clone.join("specs/specdev/schemas/inlined/blocks/embassy.schema.yaml").is_file());
}
