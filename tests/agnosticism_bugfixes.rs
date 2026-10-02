//! Bug + UX fixes from the zfm audio-plugin dogfood
//! (F-001/2/3/6/8/9/13/14/15/16/18). Each finding gets one or
//! more tests here; doc-only findings (F-008, F-015 part 1) are not
//! covered (no behavior to assert).

#[path = "common/ledger.rs"]
mod ledger;

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    report::{Finding, Report},
    validate, Severity,
};

/// The coverage rule's findings at the 2.0 template severity, error.
fn warnings_for_rule(report: &Report, rule: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

fn fresh_library(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
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

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-agnosticism-bugfixes-{tag}-{}-{}",
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

fn fresh_audio_plugin(tag: &str, project_name: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "audio-plugin".into(),
            language: "rust".into(),
            project_name: project_name.into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn errors_with_prefix(report: &Report, rule_prefix: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule.starts_with(rule_prefix) && matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

// ===========================================================================
// F-001 — {{project_name}} placeholder is substituted in
//         architecture.toml.template; schema rejects literal {{...}} leaks.
// ===========================================================================

#[test]
fn audio_plugin_architecture_template_substitutes_project_name() {
    let (target, _) = fresh_audio_plugin("f001-substitute", "my_awesome_synth");
    let body = std::fs::read_to_string(target.join("specs/architecture.toml")).unwrap();
    assert!(
        body.contains(r#"name     = "my_awesome_synth""#),
        "expected substituted [plugin].name; got body:\n{body}"
    );
    assert!(
        !body.contains("{{project_name}}"),
        "no {{...}} placeholders should leak through bootstrap; got body:\n{body}"
    );
}

// ===========================================================================
// F-003 — requirements_have_satisfiers collapses to a summary on
//          zero-module projects; per-line on populated trees.
// ===========================================================================

#[test]
fn requirements_have_satisfiers_collapses_on_zero_modules() {
    let (target, _layout) = fresh_library("f003-collapse");
    ledger::write(&target, "requirements.toml", r#"
[[requirement]]
id = "ZFM-001"
text = "First."

[[requirement]]
id = "ZFM-002"
text = "Second."

[[requirement]]
id = "ZFM-003"
text = "Third."
"#);
    let report = validate::run_project(&target, None).unwrap();
    let warns = warnings_for_rule(&report, "requirements_have_satisfiers");
    assert_eq!(
        warns.len(),
        1,
        "expected one summary warning on zero-module project; got {}:\n{}",
        warns.len(),
        report.render_plain()
    );
    let msg = &warns[0].message;
    assert!(
        msg.contains("3 requirements") && msg.contains("zero modules"),
        "summary must name the count and zero-module condition; got: {msg}"
    );
}

#[test]
fn requirements_have_satisfiers_per_line_when_modules_present() {
    let (target, _layout) = fresh_library("f003-perline");
    ledger::write(&target, "requirements.toml", r#"
[[requirement]]
id = "ZFM-001"
text = "First."

[[requirement]]
id = "ZFM-002"
text = "Second."
"#);
    // One module that satisfies neither — both requirements still
    // orphan, but the project has Module entities, so we want the
    // per-line shape (not the summary).
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let warns = warnings_for_rule(&report, "requirements_have_satisfiers");
    assert_eq!(
        warns.len(),
        2,
        "expected per-requirement warnings on populated tree; got:\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// F-018 — `gen --strict --module X` writes the assembled prompt to stdout.
// ===========================================================================

// ===========================================================================
// F-016 — `[[bin]]` standalone row paired with stub `src/bin/standalone.rs`.
// ===========================================================================

// ===========================================================================
// F-014 — coverage report reads `[[requirement]]` (singular per the schema)
//          and emits a populated coverage matrix. Originally surfaced via
//          `gen --skeleton` HANDOFF.md; coverage logic now lives in
//          `report coverage`. Same regression guarded.
// ===========================================================================

#[test]
fn coverage_report_populates_matrix_from_singular_requirement_key() {
    use specdev_analyzer::coverage;

    let (target, layout) = fresh_library("f014-coverage");
    ledger::write(&target, "requirements.toml", r#"
[[source]]
id = "rfc"
title = "Project RFC"

[[requirement]]
id = "REQ-API-001"
text = "Public types must implement Debug."
source = "rfc"

[[requirement]]
id = "REQ-API-002"
text = "Errors carry context."
source = "rfc"
"#);
    std::fs::write(
        target.join("specs/modules/Core.toml"),
        r#"name = "Core"
visibility = "pub"
satisfies = ["REQ-API-001"]

[[items]]
kind = "fn"
name = "noop"
description = "no op"
satisfies = ["REQ-API-001"]
"#,
    )
    .unwrap();

    let _ = layout;
    let layout = LayoutContext::discover(&target, None).unwrap();
    let report = coverage::build(&layout).expect("coverage");
    let yaml = coverage::render_yaml(&report).expect("yaml render");

    assert!(
        yaml.contains("REQ-API-001") && yaml.contains("REQ-API-002"),
        "coverage matrix should list both requirements; yaml:\n{yaml}"
    );
    let req1 = report
        .matrix
        .iter()
        .find(|r| r.requirement == "REQ-API-001")
        .expect("REQ-API-001 row");
    assert!(
        req1.satisfied_by.iter().any(|m| m == "Core"),
        "REQ-API-001 should be satisfied by Core; got: {:?}",
        req1.satisfied_by
    );
    assert!(
        report.orphans.iter().any(|o| o == "REQ-API-002"),
        "REQ-API-002 has no satisfier and should be in orphans; got: {:?}",
        report.orphans
    );
}

// ===========================================================================
// F-009 — workspace-shape projects (no module surface) suppress
//          LAYOUT-003 warnings for module.schema.yaml + requirements.schema.yaml.
// ===========================================================================

#[test]
fn workspace_profile_suppresses_module_schema_layout_003() {
    let workspace = tmp_workspace("f009-workspace");
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "rust-workspace".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let layout_warns: Vec<&Finding> = report
        .findings()
        .iter()
        .filter(|f| f.rule == "LAYOUT-003" && matches!(f.severity, Severity::Warning))
        .collect();
    let names_module = layout_warns
        .iter()
        .any(|f| f.message.contains("module.schema.yaml"));
    let names_req = layout_warns
        .iter()
        .any(|f| f.message.contains("requirements.schema.yaml"));
    assert!(
        !names_module,
        "rust-workspace has no module surface; LAYOUT-003 should not warn for module.schema.yaml. \
         Findings:\n{}",
        report.render_plain()
    );
    assert!(
        !names_req,
        "rust-workspace has no module surface; LAYOUT-003 should not warn for requirements.schema.yaml. \
         Findings:\n{}",
        report.render_plain()
    );
}

#[test]
fn library_profile_still_warns_when_module_schema_missing() {
    let (target, _layout) = fresh_library("f009-library");
    // Delete module.schema.yaml to force the LAYOUT-003 firing path.
    std::fs::remove_file(target.join("specs/specdev/schemas/module.schema.yaml")).unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let names_module = report
        .findings()
        .iter()
        .any(|f| f.rule == "LAYOUT-003" && f.message.contains("module.schema.yaml"));
    assert!(
        names_module,
        "library-api MUST still warn when module.schema.yaml is missing. Findings:\n{}",
        report.render_plain()
    );
}

// ===========================================================================
// F-006 — `validate --quiet` suppresses info-severity lines.
// ===========================================================================

#[test]
fn validate_quiet_flag_suppresses_info_lines() {
    let (target, _layout) = fresh_library("f006-quiet");
    let bin = env!("CARGO_BIN_EXE_specdev-analyzer");

    // Default run: info lines (LAYOUT-002 et al.) are present.
    let default_out = std::process::Command::new(bin)
        .args(["validate", "--no-color", "--project"])
        .arg(&target)
        .output()
        .expect("running validate (default)");
    let default_stdout = String::from_utf8_lossy(&default_out.stdout).to_string();
    assert!(
        default_stdout.contains("info: ["),
        "default run should emit info lines; got:\n{default_stdout}"
    );

    // --quiet: info lines suppressed; summary + counts tail still emit.
    let quiet_out = std::process::Command::new(bin)
        .args(["validate", "--no-color", "--quiet", "--project"])
        .arg(&target)
        .output()
        .expect("running validate (--quiet)");
    let quiet_stdout = String::from_utf8_lossy(&quiet_out.stdout).to_string();
    let quiet_stderr = String::from_utf8_lossy(&quiet_out.stderr).to_string();
    assert!(
        !quiet_stdout.contains("info: ["),
        "--quiet should suppress info-severity render; got stdout:\n{quiet_stdout}"
    );
    assert!(
        quiet_stderr.contains("loaded ") && quiet_stderr.contains("error(s),"),
        "--quiet keeps the summary tail and counts; got stderr:\n{quiet_stderr}"
    );
}

// ===========================================================================
// F-002 part 2 — REDUCE_UNRESOLVED for bridge relations carries a
//                co-residency hint pointing at the bridge README.
// ===========================================================================

#[test]
fn bridge_reduce_unresolved_includes_co_residency_hint() {
    use specdev_analyzer::{bootstrap, inline, layout::LayoutContext};

    let workspace = tmp_workspace("f002-bridge-hint");
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "audio-plugin".into(),
            language: "rust".into(),
            project_name: "demo_plugin".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();

    inline::run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();
    inline::run_inline_block(&layout, &full_plugin(), "egui").unwrap();
    inline::run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();

    let arch = r#"[plugin]
name = "Demo"
vendor = "ExampleCo"
category = "instrument"
formats = ["vst3"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#;
    std::fs::write(target.join("specs/architecture.toml"), arch).unwrap();
    // Binding references a non-existent region — this triggers
    // REDUCE_UNRESOLVED on the bridge relation.
    std::fs::write(
        target.join("specs/modules/MyPlugin.toml"),
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "gain"
name = "Gain"
kind = "float"
min = 0.0
max = 1.0
default = 0.5

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "param_grid"
panel = "main"
purpose = "parameter_controls"

[[bindings."egui+nih-plug".parameter_widgets]]
parameter = "gain"
region = "ghost_region"
widget_kind = "knob"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let unresolved = errors_with_prefix(&report, "REDUCE_UNRESOLVED");
    assert!(
        !unresolved.is_empty(),
        "expected REDUCE_UNRESOLVED for ghost_region; got:\n{}",
        report.render_plain()
    );
    let names_bridge = unresolved
        .iter()
        .any(|f| f.message.contains("egui+nih-plug") && f.message.contains("hint:"));
    assert!(
        names_bridge,
        "expected hint: line referencing the egui+nih-plug bridge; got messages:\n{:#?}",
        unresolved.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
}

#[test]
fn audio_plugin_plugin_name_schema_rejects_literal_placeholder() {
    let (target, _) = fresh_audio_plugin("f001-schema", "demo_plugin");
    // Overwrite the substituted file with a leaked placeholder; this is
    // what F-001 saw in the wild before the substitute flip landed.
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[plugin]
name = "{{project_name}}"
vendor = "ExampleCo"
category = "instrument"
formats = ["vst3"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    let schema_errs = errors_with_prefix(&report, "SCHEMA");
    assert!(
        !schema_errs.is_empty(),
        "schema must reject literal {{...}} in [plugin].name; got:\n{}",
        report.render_plain()
    );
}
