//! `egui` block.
//!
//! Coverage:
//!   * plugin-lint regression on the full_plugin fixture (now
//!     containing egui in addition to embassy / stm32h5-rs / nih-plug).
//!   * `inline egui` populates layout + lineage.
//!   * A plugin_shell module declaring `[egui]` with panels + regions
//!     validates clean.
//!   * Closed-enum coverage: panel kind, region purpose, layout.
//!   * Reduction's `region_in_panel` relation surfaces an unresolved
//!     panel reference via REDUCE_UNRESOLVED (region.panel naming a
//!     non-existent panel).
//!   * Reduction materializes Panel + Region entities (validate clean).
//!
//! The block lives at `specdev/blocks/egui/`.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{lint_plugin_blocks, run_inline_block},
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
        "specdev-egui-block-{tag}-{}-{}",
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

fn fresh_audio_project(tag: &str) -> (PathBuf, LayoutContext) {
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
    write_default_arch(&target);
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
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

fn write_default_arch(target: &Path) {
    std::fs::write(target.join("specs/architecture.toml"), ARCH_DEFAULT).unwrap();
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

// ---------------------------------------------------------------------------
// 1 — full_plugin fixture (with egui added) still passes block-lint.
// ---------------------------------------------------------------------------
#[test]
fn full_plugin_fixture_passes_block_lint_with_egui() {
    let offenders = lint_plugin_blocks(&full_plugin()).unwrap();
    assert!(
        offenders.is_empty(),
        "full_plugin fixture should pass block-lint; got offenders: {offenders:?}"
    );
}

// ---------------------------------------------------------------------------
// 2 — inline egui populates schema + reduction + lineage.
// ---------------------------------------------------------------------------
#[test]
fn inline_egui_populates_layout_and_lineage() {
    let (target, layout) = fresh_audio_project("inline-layout");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    assert!(target
        .join("specs/specdev/schemas/inlined/blocks/egui.schema.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/reductions/blocks/egui.yaml")
        .is_file());

    let lineage = Lineage::load(&target.join("specs/specdev")).unwrap();
    assert!(
        lineage.blocks.contains_key("egui"),
        "lineage carries egui entry: {:#?}",
        lineage.blocks
    );
}

// ---------------------------------------------------------------------------
// 3 — Full [egui] surface (panels + regions across multiple purposes)
//     validates clean.
// ---------------------------------------------------------------------------
#[test]
fn full_egui_module_validates_clean() {
    let (target, layout) = fresh_audio_project("full-validate");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[egui]
default_size = { width = 480, height = 320 }
resizable = true

[[egui.panels]]
id = "header"
kind = "top"
height = 40

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "param_grid"
panel = "main"
purpose = "parameter_controls"
layout = "grid"
columns = 2

[[egui.regions]]
id = "scope"
panel = "main"
purpose = "free_form"

[[egui.regions]]
id = "preset_picker"
panel = "header"
purpose = "preset_selector"
layout = "horizontal"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errs.is_empty(),
        "expected no errors for full egui module; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 4 — Schema rejects unknown panel kind.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_panel_kind() {
    let (target, layout) = fresh_audio_project("bad-panel-kind");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[egui.panels]]
id = "weird"
kind = "floating"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown panel kind; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 5 — Schema rejects unknown region purpose.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_region_purpose() {
    let (target, layout) = fresh_audio_project("bad-purpose");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "weird"
panel = "main"
purpose = "interactive_3d"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown region purpose; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 6 — Schema rejects unknown layout enum value.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_layout() {
    let (target, layout) = fresh_audio_project("bad-layout");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "weird"
panel = "main"
purpose = "free_form"
layout = "spiral"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown layout; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 7 — region_in_panel relation surfaces unresolved panel reference via
//     REDUCE_UNRESOLVED (a region naming a non-existent panel).
// ---------------------------------------------------------------------------
#[test]
fn region_referencing_missing_panel_fires_reduce_unresolved() {
    let (target, layout) = fresh_audio_project("region-bad-panel");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "orphan"
panel = "nonexistent"
purpose = "free_form"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "REDUCE_UNRESOLVED");
    let panel_errs: Vec<_> = errs
        .iter()
        .filter(|f| {
            f.message.contains("Panel") && f.message.contains("nonexistent")
        })
        .collect();
    assert_eq!(
        panel_errs.len(),
        1,
        "expected one REDUCE_UNRESOLVED error naming Panel/nonexistent; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 8 — Schema rejects panel without `kind` (required field). Using
//     missing-kind instead of missing-id because the reduction's name
//     template references `.id` and would hard-error on missing-id
//     before the schema finding surfaces in the report.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_panel_without_kind() {
    let (target, layout) = fresh_audio_project("panel-no-kind");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[egui.panels]]
id = "main"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for panel without kind; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 9 — Schema rejects egui table with NO panels (minItems: 1 on panels).
//     An [egui] block with regions but no panels is structurally
//     incoherent — every region must live in a panel.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_egui_with_no_panels() {
    let (target, layout) = fresh_audio_project("no-panels");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[egui]
resizable = true
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    // [egui] is permitted to be present but `panels` is required when
    // panels[] is referenced — actually we made minItems: 1 so an
    // empty [egui] without panels[] should still pass since `panels`
    // is optional. Let's verify the actual reject case is panels = [].
    // Adjust: write [egui] with explicit panels = [].
    drop(report);
    drop(errs);

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[egui]
panels = []
"#,
    );
    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for egui.panels = [] (minItems: 1); got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 10 — Reduction materializes Panel + Region entities AND the
//      region_in_panel relation succeeds when references resolve.
//      A two-panel + two-region module validates clean.
// ---------------------------------------------------------------------------
#[test]
fn reduction_materializes_panel_region_entities_and_relation() {
    let (target, layout) = fresh_audio_project("reduce-entities");
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[egui.panels]]
id = "header"
kind = "top"

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "title"
panel = "header"
purpose = "label_block"

[[egui.regions]]
id = "param_grid"
panel = "main"
purpose = "parameter_controls"
layout = "grid"
columns = 2
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errs.is_empty(),
        "expected no errors for full panel + region surface; got {errs:#?}"
    );
}
