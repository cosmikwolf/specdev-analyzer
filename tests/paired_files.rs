//! Profile-declared `paired_files` — written once, when absent, by
//! `bootstrap`; a bad row is refused before bootstrap writes anything.
//!
//! The `nextjs-app` fixture profile declares the rows listed in `PAIRED`.

use std::fs;
use std::path::{Path, PathBuf};

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

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

/// A copy of the test plugin whose `nextjs-app` profile carries one extra
/// `paired_files` row.
fn plugin_with_row(tag: &str, extra_row: &str) -> PathBuf {
    let root = tmp_workspace(&format!("{tag}-plugin")).join("_plugin");
    copy_dir(&full_plugin(), &root);
    let p = root.join("profiles/nextjs-app/profile.yaml");
    let body = fs::read_to_string(&p).unwrap();
    assert!(
        body.contains("paired_files:\n"),
        "fixture profile declares paired_files"
    );
    fs::write(
        &p,
        body.replace(
            "paired_files:\n",
            &format!("paired_files:\n  - {extra_row}\n"),
        ),
    )
    .unwrap();
    root
}

fn bootstrap_into(plugin: &Path, target: &Path) -> anyhow::Result<()> {
    bootstrap::run_bootstrap(
        plugin,
        &BootstrapInputs {
            profile: "nextjs-app".into(),
            language: "typescript".into(),
            project_name: "hello_next".into(),
            target_dir: target.to_path_buf(),
        },
    )
}

/// Every `paired_files` target the `nextjs-app` profile declares, in
/// declaration order. This list is the ONE place the set is written down:
/// the test below compares the profile's own declarations against it, so a
/// new entry needs no second edit and no count in a test name (#140 added
/// `pnpm-workspace.yaml` and had to change both).
const PAIRED: &[&str] = &[
    "tsconfig.json",
    "next.config.ts",
    "vitest.config.ts",
    "playwright.config.ts",
    ".gitignore",
    "app/.gitkeep",
    // pnpm 10 refuses a dependency's install scripts by default and still
    // exits 0; the allowlist lives in this file.
    "pnpm-workspace.yaml",
    // The CI workflow every profile ships: it runs scripts/specdev-gates.sh.
    ".github/workflows/specdev.yml",
];

#[test]
fn nextjs_app_paired_files_match_the_declared_set_and_bootstrap_copies_every_template() {
    let project = fresh_nextjs_app("decl");
    let profile = Profile::load(&project.join("specs/specdev/profile.yaml")).unwrap();
    let declared: Vec<&str> = profile
        .paired_files
        .iter()
        .map(|r| r.target.as_str())
        .collect();
    assert_eq!(
        declared, PAIRED,
        "the profile's paired_files targets and this test's PAIRED list \
         disagree. Update PAIRED — it is the single place the set is \
         written down, and every other assertion in this file reads it."
    );
    for row in &profile.paired_files {
        let t = project.join("specs/specdev").join(&row.template_file);
        assert!(t.is_file(), "bootstrap must copy {}", t.display());
    }
}

#[test]
fn profile_without_paired_files_still_loads() {
    let dir = tmp_workspace("no-paired");
    let p = dir.join("profile.yaml");
    fs::write(&p, "contract_version: 2\nname: bare\nlanguage: rust\n").unwrap();
    let profile = Profile::load(&p).unwrap();
    assert!(profile.paired_files.is_empty());
}

#[test]
fn bootstrap_writes_every_paired_file() {
    let project = fresh_nextjs_app("write");
    for rel in PAIRED {
        assert!(project.join(rel).is_file(), "{rel} missing after bootstrap");
    }
    // tsconfig.json is the post-`next build` shape with strict on.
    let ts = fs::read_to_string(project.join("tsconfig.json")).unwrap();
    assert!(ts.contains("\"strict\": true"));
    assert!(ts.contains(".next/types/**/*.ts"));
}

#[test]
fn an_existing_file_at_a_paired_target_is_kept() {
    let target = tmp_workspace("kept").join("proj");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("next.config.ts"), "// user-owned\n").unwrap();
    bootstrap_into(&full_plugin(), &target).unwrap();
    assert_eq!(
        fs::read_to_string(target.join("next.config.ts")).unwrap(),
        "// user-owned\n",
        "an existing paired file is never overwritten"
    );
    assert!(
        target.join("tsconfig.json").is_file(),
        "the absent ones are still written"
    );
}

/// Every refusal comes before bootstrap writes anything.
fn assert_refused_before_any_write(tag: &str, row: &str, needle: &str) {
    let plugin = plugin_with_row(tag, row);
    let target = tmp_workspace(tag).join("proj");
    fs::create_dir_all(&target).unwrap();
    let err = format!("{:#}", bootstrap_into(&plugin, &target).unwrap_err());
    assert!(err.contains(needle), "{err}");
    assert!(
        !target.join("specs").exists(),
        "nothing is written when a row is refused"
    );
    assert!(!target.join("spec.toml").exists());
}

#[test]
fn a_target_escaping_the_project_root_is_refused() {
    assert_refused_before_any_write(
        "escape",
        "{ template_file: templates/gitkeep.template, target: ../outside.txt }",
        "relative path inside",
    );
}

#[test]
fn a_missing_template_is_refused_naming_it() {
    assert_refused_before_any_write(
        "missing",
        "{ template_file: templates/nope.template, target: nope.txt }",
        "nope.template",
    );
}

#[test]
fn a_target_named_twice_is_refused() {
    assert_refused_before_any_write(
        "twice",
        "{ template_file: templates/gitkeep.template, target: tsconfig.json }",
        "twice",
    );
}
