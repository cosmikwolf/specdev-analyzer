//! Unit guidance: every block, bridge and profile ships a `guidance.md` —
//! stable rules for whoever edits the code, which no check can verify. It is
//! inlined like every unit file, so a project can override it and `drift`
//! sees a plugin change to it.
//!
//! 1. `bootstrap` and `inline` write the inlined copies; the profile's copy is
//!    recorded in `[profile_files]`, a unit's copy is in its upstream snapshot.
//! 2. `inline-bridge` writes the bridge's copy.
//! 3. `drift` reports a plugin change to it, and `inline --merge` carries the
//!    change (the file set is stable).
//! 4. `migrate` writes it into a tree inlined before the plugin shipped it.
//! 5. `context` lists the inlined copies.
//! 6. `plugin-lint` refuses a unit with no guidance, with fenced code in it,
//!    or over the size cap, and a block or bridge guidance that names a
//!    profile.
//! 7. The gate script tells the reader to install a command that could not
//!    start.

use crate::ci;

use std::path::{Path, PathBuf};

use ci::{plugin_root, text, tree_from};
use specdev_analyzer::{
    drift::{run_drift, FileCategory},
    inline::run_inline_bridge,
    layout::LayoutContext,
    lineage::Lineage,
    merge::run_merge_block,
    upstream::{read_upstream_unit, UnitKind},
};

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-unit-guidance-{tag}-{}-{}",
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

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}

fn units(plugin: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for kind in ["blocks", "bridges", "profiles"] {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(plugin.join(kind))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        out.extend(dirs);
    }
    out
}

fn guidance_text(unit: &Path) -> String {
    let name = unit.file_name().unwrap().to_str().unwrap();
    format!("# {name} — implementation guidance\n\n- One rule for {name}.\n")
}

/// A copy of the test plugin; with `guided`, every unit ships `guidance.md`.
fn plugin_copy(tag: &str, guided: bool) -> PathBuf {
    let root = tmp(&format!("{tag}-plugin")).join("_plugin");
    copy_dir(&plugin_root(), &root);
    for unit in units(&root) {
        let g = unit.join("guidance.md");
        if guided {
            std::fs::write(&g, guidance_text(&unit)).unwrap();
        } else {
            let _ = std::fs::remove_file(&g);
        }
    }
    root
}

fn layout(t: &ci::Tree) -> LayoutContext {
    LayoutContext::discover(&t.root, None).unwrap()
}

fn plugin_lint(plugin: &Path) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(["plugin-lint", "--plugin"])
        .arg(plugin)
        .output()
        .unwrap();
    text(&out)
}

#[test]
fn bootstrap_and_inline_write_the_inlined_guidance() {
    let plugin = plugin_copy("inline", true);
    let t = tree_from("inline", &plugin, "firmware-layered", "rust", &["rtic"]);
    assert_eq!(
        t.read("specs/specdev/guidance/profile.md"),
        guidance_text(&plugin.join("profiles/firmware-layered"))
    );
    assert_eq!(
        t.read("specs/specdev/guidance/blocks/rtic.md"),
        guidance_text(&plugin.join("blocks/rtic"))
    );
    let lineage = Lineage::load(&t.root.join("specs/specdev")).unwrap();
    assert!(
        lineage
            .profile_files
            .contains_key("specs/specdev/guidance/profile.md"),
        "the profile guidance is recorded: {:?}",
        lineage.profile_files.keys().collect::<Vec<_>>()
    );
    let snap = read_upstream_unit(&t.root.join("specs/specdev"), UnitKind::Block, "rtic").unwrap();
    assert!(
        snap.contains_key(Path::new("guidance.md")),
        "in the upstream snapshot: {:?}",
        snap.keys()
    );
}

#[test]
fn inline_bridge_writes_the_bridge_guidance() {
    let plugin = plugin_copy("bridge", true);
    let t = tree_from(
        "bridge",
        &plugin,
        "audio-plugin",
        "rust",
        &["egui", "nih-plug"],
    );
    run_inline_bridge(&layout(&t), &plugin, "egui+nih-plug").unwrap();
    assert_eq!(
        t.read("specs/specdev/guidance/bridges/egui+nih-plug.md"),
        guidance_text(&plugin.join("bridges/egui+nih-plug"))
    );
}

#[test]
fn drift_reports_and_merge_carries_a_guidance_change() {
    let plugin = plugin_copy("merge", true);
    let t = tree_from("merge", &plugin, "firmware-layered", "rust", &["rtic"]);
    let changed = "# rtic — implementation guidance\n\n- A changed rule.\n";
    std::fs::write(plugin.join("blocks/rtic/guidance.md"), changed).unwrap();

    let report = run_drift(&layout(&t), &plugin).unwrap();
    assert!(
        report.invariants.is_empty(),
        "no invariant: {:?}",
        report.invariants
    );
    let rtic = report
        .units
        .iter()
        .find(|u| u.name == "rtic")
        .expect("rtic in the report");
    let g = rtic
        .files
        .iter()
        .find(|f| f.rel_in_unit.ends_with("guidance.md"))
        .expect("guidance in the report");
    assert!(
        matches!(g.category, FileCategory::PluginMoved),
        "plugin moved: {:?}",
        g.category
    );

    let outcome = run_merge_block(&layout(&t), &plugin, "rtic").unwrap();
    assert!(outcome.clean, "the merge is clean");
    assert_eq!(t.read("specs/specdev/guidance/blocks/rtic.md"), changed);
}

#[test]
fn migrate_writes_guidance_into_a_tree_inlined_before_it_existed() {
    let plugin = plugin_copy("migrate", false);
    let t = tree_from("migrate", &plugin, "firmware-layered", "rust", &["rtic"]);
    assert!(!t.root.join("specs/specdev/guidance").exists());
    for unit in units(&plugin) {
        std::fs::write(unit.join("guidance.md"), guidance_text(&unit)).unwrap();
    }
    let out = t.analyzer(&["migrate"]);
    assert!(out.status.success(), "migrate:\n{}", text(&out));
    assert!(
        t.root.join("specs/specdev/guidance/profile.md").is_file(),
        "{}",
        text(&out)
    );
    assert!(
        t.root
            .join("specs/specdev/guidance/blocks/rtic.md")
            .is_file(),
        "{}",
        text(&out)
    );
    let report = run_drift(&layout(&t), &plugin).unwrap();
    assert!(
        report.invariants.is_empty(),
        "no invariant: {:?}",
        report.invariants
    );
}

#[test]
fn context_lists_the_inlined_guidance() {
    let plugin = plugin_copy("context", true);
    let t = tree_from("context", &plugin, "firmware-layered", "rust", &["rtic"]);
    let out = t.analyzer(&["context", "--module", "Anything"]);
    let yaml = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        yaml.contains("specs/specdev/guidance/profile.md"),
        "context:\n{yaml}\n{}",
        text(&out)
    );
    assert!(
        yaml.contains("specs/specdev/guidance/blocks/rtic.md"),
        "context:\n{yaml}"
    );
}

#[test]
fn plugin_lint_refuses_a_unit_with_no_guidance() {
    let plugin = plugin_copy("lint-missing", true);
    std::fs::remove_file(plugin.join("blocks/rtic/guidance.md")).unwrap();
    std::fs::remove_file(plugin.join("profiles/rust-app/guidance.md")).unwrap();
    let out = plugin_lint(&plugin);
    let hits: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("[GUIDANCE-001]"))
        .collect();
    assert_eq!(hits.len(), 2, "two units lack guidance:\n{out}");
    assert!(hits.iter().any(|l| l.starts_with("rtic:")), "{out}");
    assert!(hits.iter().any(|l| l.starts_with("rust-app:")), "{out}");
}

#[test]
fn plugin_lint_refuses_fenced_code_and_an_oversized_guidance() {
    let plugin = plugin_copy("lint-shape", true);
    std::fs::write(
        plugin.join("blocks/rtic/guidance.md"),
        "# rtic\n\n```rust\nfn main() {}\n```\n",
    )
    .unwrap();
    std::fs::write(
        plugin.join("bridges/egui+nih-plug/guidance.md"),
        format!("# big\n\n{}\n", "- rule\n".repeat(700)),
    )
    .unwrap();
    let out = plugin_lint(&plugin);
    assert!(
        out.lines()
            .any(|l| l.starts_with("rtic:") && l.contains("[GUIDANCE-002]")),
        "{out}"
    );
    assert!(
        out.lines()
            .any(|l| l.starts_with("egui+nih-plug:") && l.contains("[GUIDANCE-003]")),
        "{out}"
    );
    assert!(!out.contains("[GUIDANCE-001]"), "{out}");
}

#[test]
fn plugin_lint_refuses_a_profile_name_in_block_guidance() {
    let plugin = plugin_copy("lint-profile", true);
    std::fs::write(
        plugin.join("blocks/rtic/guidance.md"),
        "# rtic\n\n- In a firmware-layered package, lock.\n",
    )
    .unwrap();
    std::fs::write(
        plugin.join("profiles/audio-plugin/guidance.md"),
        "# audio-plugin\n\n- Rules.\n",
    )
    .unwrap();
    let out = plugin_lint(&plugin);
    let hits: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("[PROSE-PROFILE-001]"))
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "one block names a profile; a profile may name itself:\n{out}"
    );
    assert!(
        hits[0].starts_with("rtic:") && hits[0].contains("guidance.md"),
        "{out}"
    );
}

#[test]
fn the_gate_script_says_to_install_a_command_that_could_not_start() {
    let plugin = plugin_copy("script", true);
    let t = tree_from("script", &plugin, "firmware-layered", "rust", &[]);
    let out = t.analyzer(&["gate", "--emit"]);
    assert!(out.status.success(), "gate --emit:\n{}", text(&out));
    let script = t.read("scripts/specdev-gates.sh");
    assert!(
        script.contains("could not start") && script.contains("install it in CI"),
        "{script}"
    );
}
