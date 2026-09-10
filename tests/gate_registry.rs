//! The gate-registry precondition on codegen: `gen --module <M> --tail`
//! and whole-prompt `gen --module <M>` refuse a project whose inlined
//! `profile.yaml` declares no module-scope gate, and the refusal text
//! carries either the plugin's registry (paste block) or the minimal
//! shape to write. `report gen-plan` reports the same state as a field.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir()
        .join("specdev-gate-registry-it")
        .join(format!("{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

const PROFILE_HEAD: &str = "name: test-profile\nlanguage: rust\n";
const MODULE_GATE: &str =
    "gates:\n  - label: check\n    scope: module\n    steps: [[cargo, check, --quiet]]\n";
const TWO_GATES: &str = "gates:\n  - label: fast\n    scope: module\n    steps:\n      - [pnpm, install]\n      - [pnpm, exec, tsc, --noEmit]\n  - label: target-check\n    scope: module\n    steps: [[cargo, check, --target, \"${root.platform.target_triple}\"]]\n  - label: build\n    scope: batch\n    steps: [[pnpm, exec, next, build]]\n";
const BATCH_ONLY: &str =
    "gates:\n  - label: build\n    scope: batch\n    steps: [[cargo, build]]\n";

/// A project with one module and an inlined profile carrying `gates`
/// (`None` = no `profile.yaml` at all).
fn project(root: &Path, gates: Option<&str>) {
    fs::create_dir_all(root.join("specs/specdev")).unwrap();
    fs::create_dir_all(root.join("specs/modules")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("spec.toml"),
        "spec_version = \"1.0\"\n\n[project]\nname = \"gatefixture\"\nprofile = \"test-profile\"\nlanguage = \"rust\"\n\n[paths]\nspecs = \"specs\"\nsource = \"src\"\n",
    )
    .unwrap();
    fs::write(
        root.join("specs/modules/Widget.toml"),
        "name = \"Widget\"\nlayer = \"driver\"\n",
    )
    .unwrap();
    if let Some(g) = gates {
        fs::write(
            root.join("specs/specdev/profile.yaml"),
            format!("{PROFILE_HEAD}{g}"),
        )
        .unwrap();
    }
}

/// A plugin root whose `profiles/test-profile/profile.yaml` carries
/// `gates` (`None` = no such profile directory).
fn plugin(root: &Path, gates: Option<&str>) -> PathBuf {
    let p = root.join("plugin");
    fs::create_dir_all(p.join(".claude-plugin")).unwrap();
    fs::write(p.join(".claude-plugin/plugin.json"), "{}\n").unwrap();
    if let Some(g) = gates {
        fs::create_dir_all(p.join("profiles/test-profile")).unwrap();
        fs::write(
            p.join("profiles/test-profile/profile.yaml"),
            format!("{PROFILE_HEAD}{g}"),
        )
        .unwrap();
    }
    p
}

fn run(project: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(args)
        .arg("--project")
        .arg(project)
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

fn gen_tail(project: &Path, plugin: &Path) -> (i32, String, String) {
    run(
        project,
        &[
            "gen",
            "--module",
            "Widget",
            "--tail",
            "--plugin-root",
            plugin.to_str().unwrap(),
        ],
    )
}

#[test]
fn a_declared_module_gate_lets_gen_assemble_the_tail() {
    let root = tmp("declared");
    project(&root, Some(MODULE_GATE));
    // No plugin root needed: the inlined registry answers.
    let (code, out, err) = run(&root, &["gen", "--module", "Widget", "--tail"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("Widget"), "{out}");
    let (code, out, _) = run(&root, &["report", "gen-plan"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("gates_declared: true\n"), "{out}");
}

#[test]
fn missing_registry_with_a_plugin_registry_prints_the_block_to_paste() {
    let root = tmp("stale");
    project(&root, Some(""));
    let plugin = plugin(&root, Some(TWO_GATES));
    let (code, out, err) = gen_tail(&root, &plugin);
    assert_eq!(code, 1, "{out}{err}");
    assert!(out.is_empty(), "nothing assembled: {out}");
    assert!(err.contains("declares no `gates:`"), "{err}");
    assert!(err.contains("stale"), "{err}");
    assert!(err.contains("Paste this block"), "{err}");
    // The block is the plugin registry, every row, batch included, and
    // the token survives quoted.
    assert!(err.contains("gates:\n  - label: fast\n    scope: module\n    steps:\n      - [pnpm, install]\n      - [pnpm, exec, tsc, --noEmit]\n"), "{err}");
    assert!(
        err.contains("- [cargo, check, --target, \"${root.platform.target_triple}\"]"),
        "{err}"
    );
    assert!(
        err.contains("  - label: build\n    scope: batch\n"),
        "{err}"
    );

    // Pasting the block is the fix: cut it out of stderr, append, re-run.
    let block_start = err.find("gates:\n").unwrap();
    let block_end = err[block_start..].find("\nLabels are opaque").unwrap();
    let block = &err[block_start..block_start + block_end + 1];
    let p = root.join("specs/specdev/profile.yaml");
    let body = fs::read_to_string(&p).unwrap();
    fs::write(&p, format!("{body}{block}")).unwrap();
    let (code, _, err) = gen_tail(&root, &plugin);
    assert_eq!(code, 0, "{err}");

    // Whole-prompt mode refuses the same way.
    fs::write(&p, PROFILE_HEAD).unwrap();
    let (code, _, err) = run(
        &root,
        &[
            "gen",
            "--module",
            "Widget",
            "--plugin-root",
            plugin.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("Paste this block"), "{err}");

    // `--preamble` is gate-free and unaffected.
    let (code, _, err) = run(&root, &["gen", "--preamble", "--module", "Widget"]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn absent_inlined_profile_is_the_same_refusal() {
    let root = tmp("absent");
    project(&root, None);
    let plugin = plugin(&root, Some(MODULE_GATE));
    let (code, _, err) = gen_tail(&root, &plugin);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("does not exist"), "{err}");
    assert!(err.contains("Paste this block"), "{err}");
    assert!(err.contains("re-bootstrap"), "{err}");
    let (code, out, _) = run(&root, &["report", "gen-plan"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("gates_declared: false\n"), "{out}");
}

#[test]
fn user_authored_profile_with_no_registry_anywhere_prints_the_minimal_shape() {
    let root = tmp("authored");
    project(&root, Some(""));
    // Plugin profile exists but has no registry.
    let plugin = plugin(&root, Some(""));
    let (code, _, err) = gen_tail(&root, &plugin);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("declares no module-scope gate either"),
        "{err}"
    );
    assert!(err.contains("scope: module"), "{err}");
    assert!(err.contains("- [<executable>, <argument>, ...]"), "{err}");
    assert!(!err.contains("Paste this block"), "{err}");

    // Plugin profile does not exist at all (a profile outside the plugin tree).
    fs::remove_dir_all(plugin.join("profiles")).unwrap();
    let (code, _, err) = gen_tail(&root, &plugin);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("no plugin profile exists at"), "{err}");
    assert!(err.contains("- [<executable>, <argument>, ...]"), "{err}");
}

#[test]
fn batch_only_registry_is_refused_naming_the_labels() {
    let root = tmp("batch-only");
    project(&root, Some(BATCH_ONLY));
    // No plugin root: the inlined registry exists, so the plugin is not read.
    let (code, _, err) = run(&root, &["gen", "--module", "Widget", "--tail"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("declares gates `build` but none with `scope: module`"),
        "{err}"
    );
    let (code, out, _) = run(&root, &["report", "gen-plan"]);
    assert_eq!(code, 0);
    assert!(out.starts_with("gates_declared: false\n"), "{out}");
}

#[test]
fn missing_registry_without_a_plugin_root_is_the_plugin_root_error() {
    // Not a silent pass: the check cannot complete, so it fails loudly.
    let root = tmp("no-plugin");
    project(&root, Some(""));
    let (code, _, err) = run(&root, &["gen", "--module", "Widget", "--tail"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("PLUGIN-001"), "{err}");
}

#[test]
fn the_plugin_block_loads_under_the_schema_when_pasted() {
    // The rendered block must be valid against profile.schema.yaml, or the
    // paste instruction sends the user into MANIFEST-001.
    let root = tmp("paste-valid");
    project(&root, Some(""));
    let plugin = plugin(&root, Some(TWO_GATES));
    let (_, _, err) = gen_tail(&root, &plugin);
    let start = err.find("gates:\n").unwrap();
    let end = err[start..].find("\nLabels are opaque").unwrap();
    let pasted = format!("{PROFILE_HEAD}{}", &err[start..start + end + 1]);
    let p = root.join("specs/specdev/profile.yaml");
    fs::write(&p, pasted).unwrap();
    let loaded = specdev_analyzer::manifest::Profile::load(&p).unwrap();
    assert_eq!(loaded.gates.len(), 3);
    assert_eq!(
        loaded.gates[1].steps[0][3],
        "${root.platform.target_triple}"
    );
}
