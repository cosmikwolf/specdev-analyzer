//! Pre-8.6 quick-wins cluster — F-002, F-003, F-005.
//!
//! F-002: RequirementId pattern relaxed to accept project-native ID
//!        prefixes (`ZFM-`, `RFC-`, `STORY-`, …).
//! F-003: `[PLUGIN-001]` error message reworded; full coverage of the
//!        new wording lives in plugin_root.rs unit tests, here we just
//!        assert the new key phrases appear from the bootstrap path.
//! F-005: `validate::summarize` exposes a deterministic positive-
//!        confirmation summary (modules / requirements / sources /
//!        graph_checks counts), and `[MODULE-001]` info logs fire on
//!        each module file load.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    plugin_root::{self, PluginRootInputs},
    report::{Finding, Report},
    validate, Severity,
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-quickwins-{tag}-{}-{}",
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

fn fresh_library(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn errors(report: &Report) -> Vec<&Finding> {
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect()
}

fn rule_codes(report: &Report) -> Vec<String> {
    let mut out: Vec<String> = report
        .findings()
        .iter()
        .map(|f| f.rule.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

// ===========================================================================
// F-002 — RequirementId pattern accepts project-native prefixes
// ===========================================================================

#[test]
fn requirement_id_accepts_zfm_prefix() {
    let (target, _layout) = fresh_library("f002-zfm");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[requirement]]
id = "ZFM-001"
text = "Encoder must accept a stream of MIDI bytes."

[[requirement]]
id = "RFC-7-12"
text = "Voice allocation follows priority rules."
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "ZFM- and RFC- prefixes should validate; got:\n{}",
        report.render_plain()
    );
}

#[test]
fn requirement_id_still_accepts_legacy_req_prefix() {
    let (target, _layout) = fresh_library("f002-req");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[requirement]]
id = "REQ-API-001"
text = "Public types must implement Debug."
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(errors(&report).is_empty(), "{}", report.render_plain());
}

#[test]
fn requirement_id_rejects_lowercase() {
    let (target, _layout) = fresh_library("f002-lower");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[requirement]]
id = "zfm-001"
text = "Lowercase prefix is malformed."
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let codes = rule_codes(&report);
    assert!(
        codes.iter().any(|c| c.starts_with("SCHEMA")),
        "expected SCHEMA finding for lowercase id; got: {:?}",
        codes
    );
}

#[test]
fn requirement_id_rejects_missing_number() {
    let (target, _layout) = fresh_library("f002-no-number");
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[requirement]]
id = "ZFM-INTRO"
text = "Trailing numeric segment is required."
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let codes = rule_codes(&report);
    assert!(
        codes.iter().any(|c| c.starts_with("SCHEMA")),
        "expected SCHEMA finding for missing trailing number; got: {:?}",
        codes
    );
}

// ===========================================================================
// F-003 — [PLUGIN-001] wording names the inheritance gap
// ===========================================================================

#[test]
fn plugin_001_message_names_subshell_inheritance_gap() {
    // Scrub env vars that would let resolution succeed.
    let saved_specdev = std::env::var("SPECDEV_PLUGIN_ROOT").ok();
    let saved_claude = std::env::var("CLAUDE_PLUGIN_ROOT").ok();
    std::env::remove_var("SPECDEV_PLUGIN_ROOT");
    std::env::remove_var("CLAUDE_PLUGIN_ROOT");

    let result = plugin_root::resolve_for_bootstrap(&PluginRootInputs::default());

    if let Some(v) = saved_specdev {
        std::env::set_var("SPECDEV_PLUGIN_ROOT", v);
    }
    if let Some(v) = saved_claude {
        std::env::set_var("CLAUDE_PLUGIN_ROOT", v);
    }

    let err = result.unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("PLUGIN-001"), "{msg}");
    assert!(
        msg.contains("Bash subshells"),
        "new wording should call out Bash subshell propagation: {msg}"
    );
    assert!(
        msg.contains("export SPECDEV_PLUGIN_ROOT"),
        "new wording should tell the user to export SPECDEV_PLUGIN_ROOT: {msg}"
    );
}

// ===========================================================================
// F-005 — validate::summarize + [MODULE-001] positive-confirmation logging
// ===========================================================================

#[test]
fn summarize_reports_zero_counts_on_skeleton() {
    let (_target, layout) = fresh_library("f005-skeleton");
    let s = validate::summarize(&layout);
    assert_eq!(s.modules, 0);
    assert_eq!(s.requirements, 0);
    assert_eq!(s.sources, 0);
    // domain.toml ships graph checks; they exist even on a skeleton
    // project that hasn't authored any modules yet. Asserting >=1
    // catches a regression where the count silently drops to 0.
    assert!(
        s.graph_checks >= 1,
        "library-api ships graph checks; got {}",
        s.graph_checks
    );
}

#[test]
fn summarize_counts_modules_and_requirements() {
    let (target, layout) = fresh_library("f005-counts");
    std::fs::write(
        target.join("specs/modules/Encoder.toml"),
        r#"name = "Encoder"
visibility = "pub"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Decoder.toml"),
        r#"name = "Decoder"
visibility = "pub"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[source]]
id = "rfc-001"
title = "Project RFC"

[[source]]
id = "rfc-002"
title = "Follow-up RFC"

[[requirement]]
id = "ZFM-001"
text = "Encoder accepts MIDI."

[[requirement]]
id = "ZFM-002"
text = "Decoder produces audio."

[[requirement]]
id = "ZFM-003"
text = "Both round-trip."
"#,
    )
    .unwrap();
    let s = validate::summarize(&layout);
    assert_eq!(s.modules, 2);
    assert_eq!(s.requirements, 3);
    assert_eq!(s.sources, 2);
}

#[test]
fn summary_render_format_is_one_line_with_each_count() {
    let s = validate::Summary {
        modules: 7,
        requirements: 12,
        sources: 3,
        graph_checks: 9,
    };
    let line = s.render();
    assert_eq!(
        line,
        "loaded 7 modules / 12 requirements / 3 sources / 9 graph checks"
    );
}

#[test]
fn cli_prints_summary_tail_before_findings_count() {
    // Run the analyzer binary under a fresh skeleton library and
    // assert the stderr contains both the F-005 summary line and the
    // existing findings-count line. Order matters: summary first.
    let (target, _layout) = fresh_library("f005-cli");
    std::fs::write(
        target.join("specs/modules/Encoder.toml"),
        r#"name = "Encoder"
visibility = "pub"
"#,
    )
    .unwrap();

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(["validate", "--no-color", "--project"])
        .arg(&target)
        .output()
        .expect("running specdev-analyzer validate");
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    let summary_idx = stderr
        .find("loaded ")
        .unwrap_or_else(|| panic!("expected summary tail in stderr; got:\n{stderr}"));
    let counts_idx = stderr
        .find("error(s),")
        .unwrap_or_else(|| panic!("expected findings count in stderr; got:\n{stderr}"));
    assert!(
        summary_idx < counts_idx,
        "summary should print before findings count; got:\n{stderr}"
    );
    assert!(
        stderr.contains("1 modules"),
        "summary must reflect the one-module project; got:\n{stderr}"
    );
}
