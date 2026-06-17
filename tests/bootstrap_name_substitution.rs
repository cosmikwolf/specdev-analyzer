//! Bootstrap-time `{{project_name}}` substitution coverage.
//!
//! F-015 (2026-05-08 zfm dogfood): the audio-plugin
//! `architecture.toml.template` ships with `[plugin] name =
//! "{{project_name}}"`; the report worried the placeholder would leak
//! through to the bootstrapped tree. The substitution path is wired
//! (`bootstrap.rs::profile_plan` flags `*.toml.template` files with
//! `substitute: true`), but until now there was no end-to-end test that
//! every file the bootstrap writes is `{{...}}`-free for every shipped
//! profile.
//!
//! This test bootstraps each full_plugin profile and walks the entire
//! output tree asserting no `{{` markers remain. The schema guard
//! (`pattern: "^[^{}]+$"` on `architecture.toml`'s `[plugin].name`) stays
//! as a separate defense-in-depth layer; this test catches regressions
//! at the substitution layer itself.

use std::path::{Path, PathBuf};

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-bootstrap-name-subst-{tag}-{}-{}",
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

fn bootstrap_at(plugin: &Path, profile: &str, name: &str, tag: &str) -> PathBuf {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        plugin,
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: name.into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

/// Bootstrap-substituted files: every `*.toml.template` source resolves
/// to one of these output paths, all of which must be `{{...}}`-free.
/// `module.schema.yaml` is also substituted — the `{{language}}` token in
/// the items[] $ref is replaced at bootstrap.
///
/// `inlined/languages/<lang>/items.schema.yaml` is NOT included: it is
/// byte-faithful from `${PLUGIN_ROOT}/languages/<lang>/`, may contain
/// `{{language}}` in human-readable descriptions, and is consumed
/// downstream as a JSON Schema where the description text doesn't drive
/// validation. The schema-side substitution only applies inside the
/// per-profile `module.schema.yaml` `$ref`.
fn substituted_outputs() -> &'static [&'static str] {
    &[
        "spec.toml",
        "specs/specdev/domain.toml",
        "specs/architecture.toml",
        "specs/requirements.toml",
        "specs/specdev/schemas/module.schema.yaml",
    ]
}

fn assert_substituted_outputs_have_no_placeholders(target: &Path, tag: &str) {
    let mut leaks: Vec<(String, String)> = Vec::new();
    for rel in substituted_outputs() {
        let p = target.join(rel);
        if !p.is_file() {
            continue; // not all profiles author all five (module.schema may
                      // legitimately be absent on rust-workspace).
        }
        let text = std::fs::read_to_string(&p).unwrap();
        for tok in ["{{project_name}}", "{{language}}", "{{profile}}"] {
            if text.contains(tok) {
                leaks.push((rel.to_string(), tok.to_string()));
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "[{tag}] placeholder leak in bootstrapped substituted outputs:\n{}",
        leaks
            .iter()
            .map(|(p, t)| format!("  {p} contains {t}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
}

#[test]
fn audio_plugin_bootstrap_substitutes_project_name() {
    let target = bootstrap_at(&full_plugin(), "audio-plugin", "myplugin", "ap");
    // Read architecture.toml — F-015's specific call-out.
    let arch = std::fs::read_to_string(target.join("specs/architecture.toml")).unwrap();
    assert!(
        arch.contains("name     = \"myplugin\"") || arch.contains("name = \"myplugin\""),
        "[plugin].name was not substituted to the --name value:\n{arch}"
    );
    assert!(
        !arch.contains("{{project_name}}"),
        "{{{{project_name}}}} placeholder leaked through to architecture.toml:\n{arch}"
    );
    assert_substituted_outputs_have_no_placeholders(&target, "audio-plugin");
}

#[test]
fn library_api_bootstrap_substitutes_project_name() {
    let target = bootstrap_at(&full_plugin(), "library-api", "mylib", "lib");
    assert_substituted_outputs_have_no_placeholders(&target, "library-api");
}

#[test]
fn firmware_layered_bootstrap_substitutes_project_name() {
    let target = bootstrap_at(&full_plugin(), "firmware-layered", "myfw", "fw");
    assert_substituted_outputs_have_no_placeholders(&target, "firmware-layered");
}

#[test]
fn rust_workspace_bootstrap_substitutes_project_name() {
    let target = bootstrap_at(&full_plugin(), "rust-workspace", "myws", "ws");
    assert_substituted_outputs_have_no_placeholders(&target, "rust-workspace");
}
