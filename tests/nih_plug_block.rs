//! `nih-plug` block.
//!
//! Coverage:
//!   * `plugin-lint blocks` extension still passes on the
//!     full_plugin fixture now that nih-plug is in it (regression).
//!   * `inline nih-plug` into a bootstrapped audio-plugin project
//!     populates the expected files + lineage entry.
//!   * A plugin_shell module declaring `[nih_plug]` with parameters
//!     (float / int / bool / enum), audio ports, MIDI singleton, and
//!     process subtable validates clean.
//!   * Closed-enum coverage rejects unknown values for kind, direction,
//!     audio port kind, smoother, process mode.
//!   * Discriminated-union enforcement rejects mismatched-kind extras
//!     (e.g. a `kind = "bool"` parameter cannot carry `min` / `max`).
//!
//! The block lives at `specdev/blocks/nih-plug/`.

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
        "specdev-nih-plug-block-{tag}-{}-{}",
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
// 1 — plugin-lint regression: full_plugin fixture (now containing
//     nih-plug) still passes lint with zero offenders.
// ---------------------------------------------------------------------------
#[test]
fn full_plugin_fixture_passes_block_lint_with_nih_plug() {
    let offenders = lint_plugin_blocks(&full_plugin()).unwrap();
    assert!(
        offenders.is_empty(),
        "full_plugin fixture should pass block-lint; got offenders: {offenders:?}"
    );
}

// ---------------------------------------------------------------------------
// 2 — inline nih-plug populates schema + reduction + lineage.
// ---------------------------------------------------------------------------
#[test]
fn inline_nih_plug_populates_layout_and_lineage() {
    let (target, layout) = fresh_audio_project("inline-layout");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    assert!(
        target
            .join("specs/specdev/schemas/inlined/blocks/nih-plug.schema.yaml")
            .is_file(),
        "schema inlined under inlined/blocks/"
    );
    assert!(
        target
            .join("specs/specdev/reductions/blocks/nih-plug.yaml")
            .is_file(),
        "reduction inlined under reductions/blocks/"
    );

    let lineage = Lineage::load(&target.join("specs/specdev")).unwrap();
    assert!(
        lineage.blocks.contains_key("nih-plug"),
        "lineage carries nih-plug entry: {:#?}",
        lineage.blocks
    );
}

// ---------------------------------------------------------------------------
// 3 — A plugin_shell module with a full [nih_plug] surface validates clean.
// ---------------------------------------------------------------------------
#[test]
fn full_nih_plug_module_validates_clean() {
    let (target, layout) = fresh_audio_project("full-validate");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
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
smoother_ms = 50

[[nih_plug.parameters]]
id = "voices"
name = "Voice Count"
kind = "int"
min = 1
max = 32
default = 8

[[nih_plug.parameters]]
id = "bypass"
name = "Bypass"
kind = "bool"
default = false

[[nih_plug.parameters]]
id = "filter_type"
name = "Filter"
kind = "enum"
variants = ["lowpass", "highpass", "bandpass", "notch"]
default = "lowpass"

[[nih_plug.audio_ports]]
name = "main_in"
direction = "input"
channel_count = 2
kind = "main"

[[nih_plug.audio_ports]]
name = "main_out"
direction = "output"
channel_count = 2
kind = "main"

[nih_plug.midi]
input_level = "midi_ccs"
output_level = "none"

[nih_plug.process]
mode = "realtime"
block_size_max = 4096
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
        "expected no errors for full nih_plug module; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 4 — Schema rejects parameter with unknown kind.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_parameter_with_unknown_kind() {
    let (target, layout) = fresh_audio_project("bad-param-kind");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "weird"
name = "Weird"
kind = "complex_number"
min = 0.0
max = 1.0
default = 0.5
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown parameter kind; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 5 — Discriminated union: a bool parameter cannot carry min/max
//     (additionalProperties: false on the BoolParameter branch).
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_bool_parameter_with_min_max() {
    let (target, layout) = fresh_audio_project("bool-with-range");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "bypass"
name = "Bypass"
kind = "bool"
default = false
min = 0.0
max = 1.0
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for bool parameter with min/max; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 6 — Schema rejects audio_port with unknown direction.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_audio_port_with_unknown_direction() {
    let (target, layout) = fresh_audio_project("bad-port-direction");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.audio_ports]]
name = "main_in"
direction = "bidirectional"
channel_count = 2
kind = "main"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown audio_port direction; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 7 — Schema rejects unknown smoother enum.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_smoother() {
    let (target, layout) = fresh_audio_project("bad-smoother");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

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
smoother = "cubic"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown smoother; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 8 — Schema rejects unknown process mode.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_process_mode() {
    let (target, layout) = fresh_audio_project("bad-process-mode");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[nih_plug.process]
mode = "deferred"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown process mode; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 9 — Schema rejects enum parameter with fewer than 2 variants
//     (minItems: 2 on EnumParameter.variants).
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_enum_parameter_with_one_variant() {
    let (target, layout) = fresh_audio_project("enum-one-variant");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "single"
name = "Single"
kind = "enum"
variants = ["only"]
default = "only"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for enum with single variant; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 10 — ParameterId pattern: rejects PascalCase / non-snake_case ids.
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_pascal_case_parameter_id() {
    let (target, layout) = fresh_audio_project("pascal-id");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "Gain"
name = "Gain"
kind = "float"
min = 0.0
max = 1.0
default = 0.5
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for PascalCase parameter id (must be snake_case); got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 11 — Reduction creates Parameter / AudioPort / MidiConfig entities.
//      Verify by triggering a `module_satisfies_resolves` *style* check:
//      reference an unknown parameter as a sibling-module dep should
//      surface (NOT testing the relation itself; just confirming
//      reduction execution doesn't error on the materialized entities).
// ---------------------------------------------------------------------------
#[test]
fn reduction_materializes_parameter_audio_port_midi_entities() {
    let (target, layout) = fresh_audio_project("reduce-entities");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

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

[[nih_plug.audio_ports]]
name = "main_in"
direction = "input"
channel_count = 2

[nih_plug.midi]
input_level = "basic"
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
        "expected no errors during reduction execution; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 12 — A `[nih_plug.midi.sysex]` subtable validates clean.
//      SysEx support is delivered at MidiConfig::Basic (input = true);
//      the subtable carries the buffer size codegen needs.
// ---------------------------------------------------------------------------
#[test]
fn sysex_subtable_validates_clean() {
    let (target, layout) = fresh_audio_project("sysex-clean");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[nih_plug.midi]
input_level = "basic"

[nih_plug.midi.sysex]
max_message_len = 4104
message_type = "Dx7Dump"
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
        "expected no errors for a valid sysex subtable; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 13 — `[nih_plug.midi.sysex]` without `max_message_len` is rejected.
//      The buffer size is a required field — codegen can't emit a
//      `type Buffer = [u8; N]` array length without it.
// ---------------------------------------------------------------------------
#[test]
fn sysex_subtable_missing_max_message_len_rejected() {
    let (target, layout) = fresh_audio_project("sysex-no-len");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[nih_plug.midi]
input_level = "basic"

[nih_plug.midi.sysex]
message_type = "Dx7Dump"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for sysex subtable missing max_message_len; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 14 — input_level = "midi_ccs" validates clean (the MidiConfig::MidiCCs
//      level that delivers CC + pitch-bend events).
// ---------------------------------------------------------------------------
#[test]
fn midi_ccs_level_validates_clean() {
    let (target, layout) = fresh_audio_project("midi-ccs");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[nih_plug.midi]
input_level = "midi_ccs"
output_level = "none"
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
        "expected no errors for input_level = midi_ccs; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 15 — Unknown MidiLevel value is rejected (closed enum none/basic/midi_ccs).
// ---------------------------------------------------------------------------
#[test]
fn schema_rejects_unknown_midi_level() {
    let (target, layout) = fresh_audio_project("bad-midi-level");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[nih_plug.midi]
input_level = "midi_and_mpe"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown MIDI level; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 16 — SysEx with no MIDI direction at basic+ is rejected: codegen would
//      emit a SysExMessage type no event ever reaches (the if/then guard).
//      Both levels omitted (default none) must fail — the vacuous-truth case
//      the `required` branches guard against.
// ---------------------------------------------------------------------------
#[test]
fn sysex_without_basic_direction_rejected() {
    let (target, layout) = fresh_audio_project("sysex-no-direction");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();

    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"

[nih_plug.midi]
input_level = "none"

[nih_plug.midi.sysex]
max_message_len = 4104
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for sysex with no basic+ MIDI direction; got {report:#?}"
    );
}
