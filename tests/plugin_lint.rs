//! Lock: `plugin-lint` passes clean against the live plugin tree.
//!
//! Mirrors `agnosticism_lint::live_src_passes_lint`. The analyzer test suite
//! already validates the live `specdev` plugin tree cross-repo, so running the
//! full `plugin-lint` surface (bridge lex-order + block structure + reference
//! coverage/arity) here turns `cargo test` red the moment a content regression
//! lands in the plugin tree — a wrong reference arity, a declared
//! `reference_cases` entry with no source file, a malformed block schema, etc.
//!
//! This is the enforcement gate for the reference-crate checks: hand-running
//! `plugin-lint` is no longer the only thing standing between a coverage/arity
//! violation and merge. Skipped automatically when the live plugin tree is not
//! present at the conventional sibling path (e.g. a CI run that has not checked
//! the plugin out into `./specdev`); the unit tests in `reference_lint` and
//! `inline` provide the hermetic half.

use std::path::PathBuf;

use specdev_analyzer::{inline, prose_lint, reference_lint};

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
    findings.extend(reference_lint::lint_reference_crates(&plugin).expect("lint_reference_crates"));
    findings.extend(
        prose_lint::lint_codegen_prose_profiles(&plugin).expect("lint_codegen_prose_profiles"),
    );

    assert!(
        findings.is_empty(),
        "plugin-lint findings against the live plugin tree \
         (run `specdev-analyzer plugin-lint --plugin <tree>` to reproduce):\n{}",
        findings.join("\n"),
    );
}
