//! Shipped-profile empty-but-valid gate.
//!
//! Every other profile e2e test bootstraps from a *fixture* plugin tree
//! under `tests/specs/*/_plugin`, so none of them exercise the **real
//! shipped** `profiles/*` schemas. That gap let a malformed
//! `audio-plugin/schemas/parameter_bindings.schema.yaml` (a `|` block
//! scalar broken by a one-space under-indent) ship: the file was invalid
//! YAML, never registered in the retriever, and *every* audio-plugin
//! project died at validate with `AGGREGATOR-005: resource not present`
//! — before a single module was authored.
//!
//! This test closes that gap. For **each** profile the plugin actually
//! ships, it runs a real on-disk `bootstrap` (empty project) against the
//! real plugin root, then `validate`, and asserts **0 errors** — the
//! empty-but-valid invariant. A broken shipped schema now fails here the
//! moment it lands, and every future profile is covered automatically
//! (the profile list is discovered from disk, not hard-coded).
//!
//! Its companion `malformed_schema_surfaces_located_error_not_silent_skip`
//! guards the other half: the aggregator now surfaces a located parse
//! error instead of silently dropping a malformed schema, so when this
//! gate fails it points straight at the bad file and line.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    report::{Finding, Report},
    validate, Severity,
};

/// The real, shipped plugin tree — a sibling of the analyzer crate
/// (`agentic_tools/specdev`). NOT a `tests/specs/*` fixture.
fn shipped_plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("analyzer crate has a parent dir")
        .join("specdev")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-shipped-profiles-{tag}-{}-{}",
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

fn errors(report: &Report) -> Vec<&Finding> {
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect()
}

/// Discover the profiles the plugin ships, straight from disk, so this
/// gate automatically covers any profile added later.
fn shipped_profiles(plugin_root: &Path) -> Vec<String> {
    let profiles_dir = plugin_root.join("profiles");
    let mut names: Vec<String> = std::fs::read_dir(&profiles_dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", profiles_dir.display()))
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    names.sort();
    names
}

#[test]
fn every_shipped_profile_bootstraps_and_validates_clean() {
    let plugin_root = shipped_plugin_root();
    assert!(
        plugin_root.join("profiles").is_dir(),
        "expected a shipped plugin tree at {} — adjust shipped_plugin_root() if the layout moved",
        plugin_root.display()
    );

    let profiles = shipped_profiles(&plugin_root);
    assert!(
        !profiles.is_empty(),
        "no profiles discovered under {}/profiles",
        plugin_root.display()
    );

    let mut failures: Vec<String> = Vec::new();

    for profile in &profiles {
        let workspace = tmp_workspace(profile);
        let target = workspace.join("proj");
        std::fs::create_dir_all(&target).unwrap();

        // Bootstrap in each profile's own declared language (a Dart
        // profile has no rust codegen tier, and vice versa).
        let language = specdev_analyzer::manifest::Profile::load(
            &plugin_root
                .join("profiles")
                .join(profile)
                .join("profile.yaml"),
        )
        .unwrap_or_else(|e| panic!("[{profile}] profile.yaml failed to load: {e:#}"))
        .language;

        if let Err(e) = bootstrap::run_bootstrap(
            &plugin_root,
            &BootstrapInputs {
                profile: profile.clone(),
                language,
                project_name: "demo".into(),
                target_dir: target.clone(),
            },
        ) {
            failures.push(format!("[{profile}] bootstrap failed: {e:#}"));
            continue;
        }

        match validate::run_project(&target, None) {
            Ok(report) => {
                let errs = errors(&report);
                if !errs.is_empty() {
                    failures.push(format!(
                        "[{profile}] pristine bootstrap must validate with 0 errors; got {}:\n{}",
                        errs.len(),
                        report.render_plain()
                    ));
                }
            }
            Err(e) => failures.push(format!("[{profile}] validate errored: {e:#}")),
        }
    }

    assert!(
        failures.is_empty(),
        "shipped profile(s) failed the empty-but-valid gate:\n\n{}",
        failures.join("\n\n")
    );
}

/// The aggregator must surface a malformed schema as a loud, located
/// parse error, never drop it silently. Before this guard a bad
/// `*.schema.yaml` under `specs/specdev/schemas/` was swallowed by an
/// `Err(_) => continue`, and the only symptom was a downstream, cryptic
/// `AGGREGATOR-005: resource not present in registry`.
#[test]
fn malformed_schema_surfaces_located_error_not_silent_skip() {
    let plugin_root = shipped_plugin_root();
    let workspace = tmp_workspace("malformed");
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &plugin_root,
        &BootstrapInputs {
            profile: "audio-plugin".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    // Corrupt a shipped schema into invalid YAML — the same failure mode
    // as the real #55 block-scalar break.
    let schema = target.join("specs/specdev/schemas/parameter_bindings.schema.yaml");
    assert!(schema.is_file(), "schema should exist after bootstrap");
    std::fs::write(
        &schema,
        "title: broken\ndescription: |\n  first line\n bad: dedent\n : : :\n",
    )
    .unwrap();

    let err = validate::run_project(&target, None)
        .expect_err("a malformed schema must fail validate loudly, not pass silently");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("parameter_bindings.schema.yaml"),
        "the error must name the offending file (located diagnostic); got:\n{msg}"
    );
}
