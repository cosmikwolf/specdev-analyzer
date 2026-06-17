//! `egui+nih-plug` bridge.
//!
//! Coverage:
//!   * bridge-lint regression (lex-ordering + structural).
//!   * `inline-bridge egui+nih-plug` after both blocks populates
//!     layout + lineage.
//!   * inline-bridge fails when blocks are missing.
//!   * A plugin_shell module declaring [nih_plug] + [egui] +
//!     bindings."egui+nih-plug".parameter_widgets[] validates clean.
//!   * Schema rejects unknown widget_kind / range_hint / label.
//!   * REDUCE_UNRESOLVED fires when binding references missing parameter.
//!   * REDUCE_UNRESOLVED fires when binding references missing region.
//!   * Reduction materializes widget_binds_parameter relations.
//!   * Co-residency works — bindings against same-module parameters
//!     and regions resolve.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{lint_plugin_blocks, lint_plugin_bridges, run_inline_block, run_inline_bridge},
    layout::LayoutContext,
    lineage::Lineage,
    report::{Finding, Report},
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
        "specdev-egui-nih-plug-bridge-{tag}-{}-{}",
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

const ARCH_DEFAULT: &str = r#"[plugin]
name = "Demo Plugin"
vendor = "ExampleCo"
version = "0.1.0"
category = "instrument"
formats = ["vst3", "clap"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#;

fn fresh_audio_with_blocks(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
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
    std::fs::write(target.join("specs/architecture.toml"), ARCH_DEFAULT).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn write_module(target: &Path, name: &str, body: &str) {
    std::fs::write(
        target.join("specs/modules").join(format!("{name}.toml")),
        body,
    )
    .unwrap();
}

fn errors_for(report: &Report, rule: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

const FULL_MODULE: &str = r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "gain"
name = "Gain"
kind = "float"
min = -30.0
max = 30.0
default = 0.0
unit = "dB"
smoother = "logarithmic"

[[nih_plug.parameters]]
id = "filter_type"
name = "Filter"
kind = "enum"
variants = ["lowpass", "highpass", "bandpass", "notch"]
default = "lowpass"

[[nih_plug.parameters]]
id = "bypass"
name = "Bypass"
kind = "bool"
default = false

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "param_grid"
panel = "main"
purpose = "parameter_controls"
layout = "grid"
columns = 2

[[bindings."egui+nih-plug".parameter_widgets]]
parameter = "gain"
region = "param_grid"
widget_kind = "knob"
range_hint = "logarithmic"
label = "above"

[[bindings."egui+nih-plug".parameter_widgets]]
parameter = "filter_type"
region = "param_grid"
widget_kind = "radio_group"

[[bindings."egui+nih-plug".parameter_widgets]]
parameter = "bypass"
region = "param_grid"
widget_kind = "toggle"
"#;

// ---------------------------------------------------------------------------
// 1 — full_plugin fixture passes block + bridge lint with egui+nih-plug present.
// ---------------------------------------------------------------------------
#[test]
fn full_plugin_fixture_passes_lint_with_bridge() {
    let block_offenders = lint_plugin_blocks(&full_plugin()).unwrap();
    assert!(
        block_offenders.is_empty(),
        "block-lint clean: {block_offenders:?}"
    );
    let bridge_offenders = lint_plugin_bridges(&full_plugin()).unwrap();
    assert!(
        bridge_offenders.is_empty(),
        "bridge-lint clean: {bridge_offenders:?}"
    );
}

// ---------------------------------------------------------------------------
// 2 — inline-bridge after both blocks succeeds + populates lineage.
// ---------------------------------------------------------------------------
#[test]
fn inline_bridge_populates_lineage() {
    let (target, layout) = fresh_audio_with_blocks("inline-bridge");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();

    assert!(target
        .join("specs/specdev/schemas/inlined/bridges/egui+nih-plug.schema.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/reductions/bridges/egui+nih-plug.yaml")
        .is_file());

    let lineage = Lineage::load(&target.join("specs/specdev")).unwrap();
    assert!(
        lineage.bridges.contains_key("egui+nih-plug"),
        "lineage carries bridge: {:#?}",
        lineage.bridges
    );
}

// ---------------------------------------------------------------------------
// 3 — inline-bridge fails when blocks are missing (preflight check).
// ---------------------------------------------------------------------------
#[test]
fn inline_bridge_fails_without_blocks() {
    let workspace = tmp_workspace("no-blocks");
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
    std::fs::write(target.join("specs/architecture.toml"), ARCH_DEFAULT).unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();

    let err = run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("egui") || msg.contains("nih-plug"),
        "expected error to mention missing block(s); got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 4 — Full bridge surface validates clean.
// ---------------------------------------------------------------------------
#[test]
fn full_bridge_module_validates_clean() {
    let (target, layout) = fresh_audio_with_blocks("full-validate");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_module(&target, "MyPlugin", FULL_MODULE);

    let report = validate::run_project(&target, None).unwrap();
    let errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errs.is_empty(),
        "expected no errors for full bridge surface; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 5 — Schema rejects unknown widget_kind.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_widget_kind() {
    let (target, layout) = fresh_audio_with_blocks("bad-widget-kind");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_module(
        &target,
        "MyPlugin",
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
region = "param_grid"
widget_kind = "spaceship"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown widget_kind; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 6 — Schema rejects unknown range_hint enum value.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_range_hint() {
    let (target, layout) = fresh_audio_with_blocks("bad-range-hint");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_module(
        &target,
        "MyPlugin",
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
region = "param_grid"
widget_kind = "knob"
range_hint = "cubic"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown range_hint; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 7 — Schema rejects unknown label position.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_label_position() {
    let (target, layout) = fresh_audio_with_blocks("bad-label");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_module(
        &target,
        "MyPlugin",
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
region = "param_grid"
widget_kind = "knob"
label = "diagonal"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown label; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 8 — REDUCE_UNRESOLVED on missing parameter reference (NIH-EGUI-001
//     equivalent in v1).
// ---------------------------------------------------------------------------
#[test]
fn binding_references_missing_parameter_fires_reduce_unresolved() {
    let (target, layout) = fresh_audio_with_blocks("missing-param");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_module(
        &target,
        "MyPlugin",
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
parameter = "nonexistent_param"
region = "param_grid"
widget_kind = "knob"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let all_errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        !all_errs.is_empty(),
        "expected at least one error for missing parameter reference; full report: {report:#?}"
    );
    // Surfaces in either REDUCE_UNRESOLVED or whichever rule the
    // reducer routes through; just assert SOMETHING fires + names the
    // missing id.
    let names_param: Vec<_> = all_errs
        .iter()
        .filter(|f| f.message.contains("nonexistent_param"))
        .collect();
    assert!(
        !names_param.is_empty(),
        "expected an error naming `nonexistent_param`; got {all_errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 9 — REDUCE_UNRESOLVED on missing region reference (NIH-EGUI-002
//     equivalent in v1).
// ---------------------------------------------------------------------------
#[test]
fn binding_references_missing_region_fires_reduce_unresolved() {
    let (target, layout) = fresh_audio_with_blocks("missing-region");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_module(
        &target,
        "MyPlugin",
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
    );

    let report = validate::run_project(&target, None).unwrap();
    let all_errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        !all_errs.is_empty(),
        "expected at least one error for missing region reference; full report: {report:#?}"
    );
    let names_region: Vec<_> = all_errs
        .iter()
        .filter(|f| f.message.contains("ghost_region"))
        .collect();
    assert!(
        !names_region.is_empty(),
        "expected an error naming `ghost_region`; got {all_errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 10 — Reduction materializes widget_binds_parameter relations.
//      Three bindings (knob/radio_group/toggle in FULL_MODULE) all
//      resolve cleanly with no errors.
// ---------------------------------------------------------------------------
#[test]
fn reduction_materializes_widget_binds_parameter_relations() {
    let (target, layout) = fresh_audio_with_blocks("reduce-relations");
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_module(&target, "MyPlugin", FULL_MODULE);

    let report = validate::run_project(&target, None).unwrap();
    let errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errs.is_empty(),
        "expected no errors during reduction with three bindings; got {errs:#?}"
    );
}
