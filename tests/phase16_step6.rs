//! Phase 16 §8.1 step 6 — `bootstrap.rs` workspace-step dispatch.
//!
//! Per doc 21 §8.1: drive workspace-shell bootstrap off `bootstrap.steps`
//! from the inlined `profile.yaml`. v1 ships the *dispatch shell* —
//! `EmitWorkspaceRoot` runs a defensive `[workspace]`-table validation,
//! `EmitMemberStubs` and `WalkMembers` are no-op v1 placeholders. Per-
//! member stub creation and recursive bootstrap land in follow-up phases
//! gated on workspace-bootstrap dogfood.
//!
//! Step 6 coverage:
//!   1. rust-workspace bootstrap completes successfully (the only profile
//!      with non-empty `bootstrap.steps`); the bootstrapped `spec.toml`
//!      retains its `[workspace]` table, so `EmitWorkspaceRoot`'s
//!      defensive check passes.
//!   2. library-api / firmware-layered / audio-plugin all bootstrap
//!      cleanly with `bootstrap.steps: []` — no dispatch fires.
//!   3. Pre-D52 phase3 fixture bootstraps without `profile.yaml`; the
//!      step dispatcher silently no-ops (no manifest → no walk).
//!   4. Defensive: rust-workspace template tampered to remove the
//!      `[workspace]` table makes `EmitWorkspaceRoot` error
//!      `BOOTSTRAP-002` with vocabulary that names the manifest field
//!      (no profile-name leak per D51).

use std::path::{Path, PathBuf};

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn phase3_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase3")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase16-step6-{tag}-{}-{}",
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

fn bootstrap_at(plugin: &Path, profile: &str, tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        plugin,
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

// ===========================================================================
// 1 — rust-workspace bootstrap fires steps cleanly (defensive
//     [workspace]-table check passes against the shipped template)
// ===========================================================================

#[test]
fn rust_workspace_bootstrap_dispatches_steps_cleanly() {
    let target = bootstrap_at(&phase6_plugin(), "rust-workspace", "ws-clean");
    // bootstrap_at would have errored if EmitWorkspaceRoot had failed;
    // the `[workspace]` table from the template must still be present.
    let spec = std::fs::read_to_string(target.join("spec.toml")).unwrap();
    assert!(
        spec.contains("[workspace]"),
        "rust-workspace template must still emit [workspace] table:\n{spec}"
    );
    assert!(target.join("specs/specdev/profile.yaml").is_file());
}

// ===========================================================================
// 2 — Non-workspace profiles bootstrap with empty bootstrap.steps; no
//     dispatch fires (and bootstrap completes without errors)
// ===========================================================================

#[test]
fn library_api_bootstrap_no_dispatch_fires() {
    let target = bootstrap_at(&phase6_plugin(), "library-api", "lib-empty");
    assert!(target.join("specs/specdev/profile.yaml").is_file());
    // No [workspace] table for library-api — the step walker would have
    // errored with BOOTSTRAP-002 if EmitWorkspaceRoot had fired.
    let spec = std::fs::read_to_string(target.join("spec.toml")).unwrap();
    assert!(!spec.contains("[workspace]"));
}

#[test]
fn firmware_layered_bootstrap_no_dispatch_fires() {
    let target = bootstrap_at(&phase6_plugin(), "firmware-layered", "fw-empty");
    assert!(target.join("specs/specdev/profile.yaml").is_file());
    let spec = std::fs::read_to_string(target.join("spec.toml")).unwrap();
    assert!(!spec.contains("[workspace]"));
}

#[test]
fn audio_plugin_bootstrap_no_dispatch_fires() {
    let target = bootstrap_at(&phase6_plugin(), "audio-plugin", "ap-empty");
    assert!(target.join("specs/specdev/profile.yaml").is_file());
    let spec = std::fs::read_to_string(target.join("spec.toml")).unwrap();
    assert!(!spec.contains("[workspace]"));
}

// ===========================================================================
// 3 — Pre-D52 fixture (no profile.yaml in profile dir) silently no-ops
//     the step dispatcher (no manifest → no walk)
// ===========================================================================

#[test]
fn pre_d52_phase3_bootstrap_no_dispatch_fires() {
    let target = bootstrap_at(&phase3_plugin(), "firmware-layered", "phase3-empty");
    assert!(
        !target.join("specs/specdev/profile.yaml").exists(),
        "phase3 fixture must NOT inline profile.yaml (test premise)"
    );
}

// ===========================================================================
// 4 — Defensive: tampered rust-workspace plugin tree (template stripped of
//     [workspace] table) makes EmitWorkspaceRoot error BOOTSTRAP-002
// ===========================================================================

#[test]
fn tampered_rust_workspace_template_errors_bootstrap_002() {
    // Build a synthetic plugin tree: copy phase6 rust-workspace verbatim,
    // then overwrite spec.toml.template with a body lacking [workspace].
    let synthetic = tmp_workspace("tampered");
    let synthetic_plugin = synthetic.join("_plugin");
    copy_dir_recursive(&phase6_plugin(), &synthetic_plugin).unwrap();
    let template = synthetic_plugin.join("profiles/rust-workspace/spec.toml.template");
    std::fs::write(
        &template,
        r#"spec_version = "1.0"

[project]
name = "{{project_name}}"
profile = "{{profile}}"
language = "{{language}}"

[paths]
specs = "specs"
source = "."
"#,
    )
    .unwrap();

    let target_workspace = tmp_workspace("tampered-target");
    let target = target_workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    let err = bootstrap::run_bootstrap(
        &synthetic_plugin,
        &BootstrapInputs {
            profile: "rust-workspace".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("BOOTSTRAP-002"),
        "expected BOOTSTRAP-002; got {msg}"
    );
    assert!(
        msg.contains("emit_workspace_root"),
        "diagnostic must reference the manifest step; got {msg}"
    );
    assert!(
        msg.contains("[workspace]"),
        "diagnostic must reference the missing [workspace] table; got {msg}"
    );
    // D51: never name a specific profile.
    assert!(
        !msg.contains("rust-workspace"),
        "diagnostic must NOT name the specific profile; got {msg}"
    );
}

// ---------------------------------------------------------------------------

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else if ft.is_file() {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
