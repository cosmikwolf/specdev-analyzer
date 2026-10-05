//! Lock: `plugin-lint` passes clean against the live plugin tree.
//!
//! Mirrors `agnosticism_lint::live_src_passes_lint`. Running the full
//! `plugin-lint` surface (bridge lex-order, block structure, guidance, rule
//! packs) here turns `cargo test` red the moment a content regression lands in
//! the plugin tree. Skipped when the live plugin tree is not present at the
//! conventional sibling path; the unit tests in `inline`, `guidance_lint` and
//! `pack_lint` provide the hermetic half.

use std::path::PathBuf;

use specdev_analyzer::{guidance_lint, inline, pack_lint};

fn live_plugin_tree() -> Option<PathBuf> {
    // Conventional layout: <workspace>/specdev next to <workspace>/specdev-analyzer.
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .join("specdev");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

#[test]
fn live_plugin_lint_passes() {
    let Some(plugin) = live_plugin_tree() else {
        eprintln!("skip: live plugin tree not present at sibling path");
        return;
    };

    let mut findings = Vec::new();
    findings.extend(inline::lint_plugin_bridges(&plugin).expect("lint_plugin_bridges"));
    findings.extend(inline::lint_plugin_blocks(&plugin).expect("lint_plugin_blocks"));
    findings.extend(
        guidance_lint::lint_guidance(&plugin).expect("lint_guidance"),
    );
    findings.extend(pack_lint::lint_packs(&plugin).expect("lint_packs"));

    assert!(
        findings.is_empty(),
        "plugin-lint findings against the live plugin tree \
         (run `specdev-analyzer plugin-lint --plugin <tree>` to reproduce):\n{}",
        findings.join("\n"),
    );
}
