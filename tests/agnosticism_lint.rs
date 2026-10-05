//! The agnosticism CI lint (`agnosticism-lint.sh`).
//!
//! Tests shell out to `.github/scripts/agnosticism-lint.sh` to verify the
//! lint's core behaviors:
//!
//!   1. Clean run against the current `src/` (post-annotation): exit 0.
//!   2. Synthesized violation in a temp src tree: non-zero exit + the
//!      offending name appears in stdout.
//!   3. Per-line allowlist marker silences a hit.
//!
//! The script is invoked with the live `specdev` plugin tree path and
//! `SPECDEV_ANALYZER_SRC` overridden to the synthesized tree where
//! relevant. If the live plugin tree is not present at the conventional
//! sibling path, the synthesized-fixture tests provide both the plugin
//! AND analyzer halves and run hermetically.
//!
//! The lint defends the analyzer-agnosticism guarantee: the analyzer holds
//! no compile-time knowledge of specific profile / block / bridge names.
//! The lint's own per-line allowlist marker token is exercised as a test
//! fixture below.

use std::path::{Path, PathBuf};
use std::process::Command;

fn analyzer_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn script_path() -> PathBuf {
    analyzer_root().join(".github/scripts/agnosticism-lint.sh")
}

fn live_plugin_tree() -> Option<PathBuf> {
    // Conventional layout: <workspace>/specdev next to <workspace>/specdev-analyzer.
    let p = analyzer_root().parent()?.join("specdev");
    if p.is_dir() {
        Some(p)
    } else {
        None
    }
}

/// Build a minimal synthetic plugin tree under `root` with a fixed set of
/// profiles/blocks/bridges. The lint sources the names from `ls` of
/// these directories; a single subdir per category is enough.
fn write_minimal_plugin_tree(root: &Path) {
    std::fs::create_dir_all(root.join("profiles/library-api")).unwrap();
    std::fs::create_dir_all(root.join("profiles/firmware-layered")).unwrap();
    std::fs::create_dir_all(root.join("blocks/embassy")).unwrap();
    std::fs::create_dir_all(root.join("bridges/embassy+stm32h5-rs")).unwrap();
    std::fs::create_dir_all(root.join("bridges/local")).unwrap();
}

fn write_synth_src(root: &Path, files: &[(&str, &str)]) {
    for (rel, body) in files {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
    }
}

fn run_lint(plugin_tree: &Path, analyzer_src_override: Option<&Path>) -> std::process::Output {
    let mut cmd = Command::new("bash");
    cmd.arg(script_path()).arg(plugin_tree);
    if let Some(src) = analyzer_src_override {
        cmd.env("SPECDEV_ANALYZER_SRC", src);
    }
    cmd.output().expect("failed to invoke agnosticism-lint.sh")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-agnosticism-lint-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Test 1: lint exits 0 against the current `src/` tree (post-annotation).
/// Skipped automatically if the live plugin tree is not
/// present at the conventional sibling path (e.g. CI runs that have not
/// yet checked out the plugin into `./specdev`).
#[test]
fn live_src_passes_lint() {
    let Some(plugin) = live_plugin_tree() else {
        eprintln!("skip: live plugin tree not present at sibling path");
        return;
    };
    let out = run_lint(&plugin, None);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0 against live src tree.\n\
         status: {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        stdout,
        stderr,
    );
    assert!(
        stdout.contains("clean. No D51 violations."), // PLANNING-OK: asserts the lint's own stdout marker
        "expected `clean.` marker in stdout; got:\n{stdout}"
    );
}

/// Test 2: a synthesized violation in a temp src tree fails the lint
/// with a non-zero exit code, and the offending name appears in stdout.
#[test]
fn synthetic_violation_fails_lint() {
    let workdir = tmp_dir("violation");
    let plugin = workdir.join("plugin");
    let synth_src = workdir.join("src");
    write_minimal_plugin_tree(&plugin);
    write_synth_src(
        &synth_src,
        &[(
            "cargo_toml.rs",
            r#"
fn route(profile: &str) -> &'static str {
    match profile {
        "firmware-layered" => "fw",
        _ => "other",
    }
}
"#,
        )],
    );

    let out = run_lint(&plugin, Some(&synth_src));
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !out.status.success(),
        "expected non-zero exit on synthetic violation; got success.\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("firmware-layered"),
        "expected offending name `firmware-layered` in stdout; got:\n{stdout}"
    );
    assert!(
        stdout.contains("D51 violation"), // PLANNING-OK: asserts the lint's own stdout header
        "expected `D51 violation` header in stdout; got:\n{stdout}" // PLANNING-OK: lint's own header string
    );
}

/// Test 3: the per-line allowlist marker silences a hit. Same source line
/// as test 2, but with the trailing comment marker — lint must exit 0.
#[test]
fn per_line_allowlist_marker_silences_hit() {
    let workdir = tmp_dir("allowlisted");
    let plugin = workdir.join("plugin");
    let synth_src = workdir.join("src");
    write_minimal_plugin_tree(&plugin);
    write_synth_src(
        &synth_src,
        &[(
            "cargo_toml.rs",
            r#"
fn route(profile: &str) -> &'static str {
    match profile {
        "firmware-layered" => "fw", // D51-fallback: test fixture; deleted in v1.1. PLANNING-OK: lint allowlist-marker fixture
        _ => "other",
    }
}
"#,
        )],
    );

    let out = run_lint(&plugin, Some(&synth_src));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "expected exit 0 with allowlist marker present.\n\
         status: {:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status.code(),
        stdout,
        stderr,
    );
}

/// Test 4: doc-string `///` and `//!` lines are filtered (legitimately
/// reference profile names for narrative purposes).
#[test]
fn doc_string_lines_are_filtered() {
    let workdir = tmp_dir("docstring");
    let plugin = workdir.join("plugin");
    let synth_src = workdir.join("src");
    write_minimal_plugin_tree(&plugin);
    write_synth_src(
        &synth_src,
        &[(
            "cargo_toml.rs",
            r#"
//! Module-level doc references "firmware-layered" for narrative purposes.

/// Outer doc references "library-api" similarly.
fn nothing() {}
"#,
        )],
    );

    let out = run_lint(&plugin, Some(&synth_src));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0 (doc-string filter must drop hits); got non-zero.\nstdout:\n{stdout}"
    );
}

/// A synthetic plugin tree that carries one file with `body`, plus an empty
/// analyzer `src/` so the name and planning checks have nothing to report.
fn removed_term_fixture(tag: &str, rel: &str, body: &str) -> (PathBuf, PathBuf) {
    let workdir = tmp_dir(tag);
    let plugin = workdir.join("plugin");
    let synth_src = workdir.join("src");
    write_minimal_plugin_tree(&plugin);
    write_synth_src(&plugin, &[(rel, body)]);
    std::fs::create_dir_all(&synth_src).unwrap();
    (plugin, synth_src)
}

/// Test 5: a removed command named in shipped plugin text fails the lint,
/// and the report names the file and the line.
#[test]
fn removed_term_in_plugin_tree_fails_lint() {
    let (plugin, src) = removed_term_fixture(
        "removed-term",
        "skills/spec-design/SKILL.md",
        "# Design\n\nWhen the spec is done, run `/spec-gen` to write the code.\n",
    );
    let out = run_lint(&plugin, Some(&src));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "expected non-zero exit for a removed term in the plugin tree; got success.\nstdout:\n{stdout}"
    );
    assert!(
        stdout.contains("skills/spec-design/SKILL.md:3:"),
        "expected the file and line of the hit in stdout; got:\n{stdout}"
    );
    assert!(
        stdout.contains("removed-term violation"),
        "expected the removed-term header in stdout; got:\n{stdout}"
    );
}

/// Test 6: the per-line escape silences a removed-term hit.
#[test]
fn removed_term_escape_silences_hit() {
    let (plugin, src) = removed_term_fixture(
        "removed-term-ok",
        "skills/spec-design/SKILL.md",
        "Older trees name `/spec-gen` in their ledgers. <!-- REMOVED-TERM-OK: names the old command -->\n",
    );
    let out = run_lint(&plugin, Some(&src));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0 with the escape present.\nstdout:\n{stdout}"
    );
}

/// Test 7: `CHANGELOG.md` records what was removed, so it is not scanned.
#[test]
fn changelog_is_exempt_from_removed_terms() {
    let (plugin, src) = removed_term_fixture(
        "removed-term-changelog",
        "CHANGELOG.md",
        "## 0.3.0\n\n- Removed `/spec-gen` and `gen-lineage.toml`.\n",
    );
    let out = run_lint(&plugin, Some(&src));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0: CHANGELOG.md is exempt.\nstdout:\n{stdout}"
    );
}
