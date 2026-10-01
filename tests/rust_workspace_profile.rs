//! `rust-workspace` profile + workspace `cargo-toml` emission (G-005).
//!
//! Three layers of coverage:
//!   * Bootstrap — `--profile rust-workspace` scaffolds the expected
//!     workspace layout (no module.schema.yaml, no module.conventions.md
//!     since workspaces have no module surface).
//!   * Workspace-root render — `[workspace]` + `[workspace.package]` +
//!     `[workspace.dependencies]` emit in idiomatic Cargo order.
//!   * Member-mode render — package fields owned by the workspace's
//!     `[workspace_package]` become `<field>.workspace = true`; deps with
//!     `source = "workspace"` render as `{ workspace = true, ... overlay
//!     ... }` (G-005 acceptance).
//!
//! Regression target: the zfm dogfood project. The `zfm_*` tests
//! reproduce its workspace `Cargo.toml` and per-member `Cargo.toml`
//! shapes.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    validate, Severity,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-rust-workspace-{tag}-{}-{}",
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

/// Bootstrap a `rust-workspace` project at a fresh temp dir; return the
/// project root. Tests overwrite spec.toml + architecture.toml to
/// provision real members + hoisted deps, then call [`discover`] to get
/// a fresh layout reflecting those overwrites. (`LayoutContext` caches
/// the parsed spec.toml at discover time, so any test that mutates
/// spec.toml after bootstrap MUST re-discover before rendering.)
fn fresh_workspace(tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "rust-workspace".into(),
            language: "rust".into(),
            project_name: "demo-ws".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn overwrite_workspace_arch_minimal(target: &Path) {
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[workspace_package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
repository = "https://example.org/demo-ws"
rust-version = "1.75"

[[workspace_dependencies]]
crate = "alpha-shared"
version = "0.1.0"
source = { path = "crates/alpha" }
default_features = false

[[workspace_dependencies]]
crate = "defmt"
version = "0.3"
"#,
    )
    .unwrap();
}

fn overwrite_spec_with_two_members_and_root_lists(
    target: &Path,
    default_members: &[&str],
    exclude: &[&str],
) {
    let quoted = |xs: &[&str]| {
        xs.iter()
            .map(|x| format!("{x:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    std::fs::write(
        target.join("spec.toml"),
        format!(
            r#"spec_version = "1.0"

[project]
name = "demo-ws"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."

[workspace]
resolver = "2"
default_members = [{}]
exclude = [{}]

[[workspace.members]]
name = "alpha-shared"
path = "crates/alpha"

[[workspace.members]]
name = "beta-utils"
path = "crates/beta"
"#,
            quoted(default_members),
            quoted(exclude),
        ),
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// 14 — The embedded spec.schema.yaml owns the `[workspace]` table: a
//      typo'd key beside `default_members` is still rejected (the
//      table stays closed), and a well-formed spec validates clean.
// ---------------------------------------------------------------------------
#[test]
fn workspace_table_stays_closed_and_valid_root_lists_validate_clean() {
    let target = fresh_workspace("ws-root-lists-valid");
    overwrite_spec_with_two_members_and_root_lists(&target, &["alpha-shared"], &["tools/bench"]);
    overwrite_workspace_arch_minimal(&target);
    let report = validate::run_project(&target, None).unwrap();
    let errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );

    let spec = std::fs::read_to_string(target.join("spec.toml")).unwrap();
    std::fs::write(
        target.join("spec.toml"),
        spec.replace("exclude = [", "excluded = ["),
    )
    .unwrap();
    let err = LayoutContext::discover(&target, None)
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default();
    assert!(
        err.contains("excluded"),
        "an unknown key under [workspace] must be rejected by the embedded schema:\n{err}"
    );
}
