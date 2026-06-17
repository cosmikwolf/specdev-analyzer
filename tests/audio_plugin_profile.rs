//! `audio-plugin` profile.
//!
//! Coverage layers:
//!   * Bootstrap — `--profile audio-plugin` scaffolds the expected
//!     layout (module + architecture + requirements + enums schemas;
//!     module.conventions.md; populated templates).
//!   * Schema validation — required `[plugin]` / `[audio]` / `[runtime]`
//!     fields, closed-enum coverage (formats, category, channel layout,
//!     panic_handler), strict SemVer on `[plugin].version`.
//!   * Layer dependency check — upward edges in the four-tier ordering
//!     (host_interface → plugin_shell → engine → dsp_node) error.
//!   * Cargo.toml render — `[lib] crate-type = ["cdylib"]` always;
//!     `[[bin]]` standalone target only when `formats` includes
//!     `"standalone"`; per-format coverage; `[features]` parity with
//!     library-api.
//!   * F-006 parity — `requirements_have_satisfiers` fires on
//!     orphan module-nature requirements.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
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
        "specdev-audio-plugin-profile-{tag}-{}-{}",
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

fn fresh_plugin(tag: &str) -> (PathBuf, LayoutContext) {
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
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

/// Default architecture.toml that satisfies all required fields. Tests
/// that need to exercise specific configurations overwrite this.
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

const ARCH_WITH_STANDALONE: &str = r#"[plugin]
name = "Demo Plugin"
vendor = "ExampleCo"
version = "0.1.0"
category = "instrument"
formats = ["vst3", "clap", "standalone"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#;

fn write_arch(target: &Path, body: &str) {
    std::fs::write(target.join("specs/architecture.toml"), body).unwrap();
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

fn warnings_for(report: &Report, rule: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && matches!(f.severity, Severity::Warning))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// 1 — Bootstrap layout: audio-plugin scaffolds the expected files.
// ---------------------------------------------------------------------------
#[test]
fn bootstrap_emits_expected_audio_plugin_layout() {
    let (target, _) = fresh_plugin("bootstrap-layout");

    // Required schemas under specs/specdev/schemas/.
    for schema in [
        "module.schema.yaml",
        "architecture.schema.yaml",
        "requirements.schema.yaml",
        "enums.schema.yaml",
    ] {
        let p = target.join("specs/specdev/schemas").join(schema);
        assert!(p.is_file(), "expected {} after bootstrap", p.display());
    }

    // Codegen prose at codegen/rust/module.conventions.md.
    assert!(
        target
            .join("specs/specdev/codegen/rust/module.conventions.md")
            .is_file()
    );

    // Project-level reduction.
    assert!(
        target
            .join("specs/specdev/reductions/project.yaml")
            .is_file()
    );

    // Templates rendered into project root.
    assert!(target.join("spec.toml").is_file());
    assert!(target.join("specs/architecture.toml").is_file());
    assert!(target.join("specs/requirements.toml").is_file());
    assert!(target.join("specs/specdev/domain.toml").is_file());

    // languages tier (items.schema.yaml inlined).
    assert!(
        target
            .join("specs/specdev/schemas/inlined/languages/rust/items.schema.yaml")
            .is_file()
    );
}

// ---------------------------------------------------------------------------
// 2 — Bootstrapped + populated default arch + minimal module = clean validate.
// ---------------------------------------------------------------------------
#[test]
fn default_bootstrap_validates_clean_with_minimal_module() {
    let (target, _) = fresh_plugin("clean-validate");
    write_arch(&target, ARCH_DEFAULT);
    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(errs.is_empty(), "expected no errors; got {errs:#?}");
}

// ---------------------------------------------------------------------------
// 3 — Schema rejects unknown layer value.
// ---------------------------------------------------------------------------
#[test]
fn module_schema_rejects_unknown_layer() {
    let (target, _) = fresh_plugin("bad-layer");
    write_arch(&target, ARCH_DEFAULT);
    write_module(
        &target,
        "Bogus",
        r#"name = "Bogus"
layer = "kernel"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA error for unknown layer value; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 4 — layer_dependency rejects upward edge: dsp_node → engine is OK,
//     engine → plugin_shell is upward and must error.
// ---------------------------------------------------------------------------
#[test]
fn layer_dependency_rejects_upward_edge() {
    let (target, _) = fresh_plugin("layer-upward");
    write_arch(&target, ARCH_DEFAULT);
    write_module(
        &target,
        "Shell",
        r#"name = "Shell"
layer = "plugin_shell"
depends_on = ["Engine"]
"#,
    );
    write_module(
        &target,
        "Engine",
        r#"name = "Engine"
layer = "engine"
depends_on = ["Shell"]
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "layer_dependency");
    assert!(
        !errs.is_empty(),
        "expected layer_dependency error on engine→plugin_shell upward edge; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 5 — Architecture schema: closed-enum `category` rejects unknown values.
// ---------------------------------------------------------------------------
#[test]
fn architecture_schema_rejects_unknown_category() {
    let (target, _) = fresh_plugin("bad-category");
    write_arch(
        &target,
        r#"[plugin]
name = "Demo"
vendor = "ExampleCo"
version = "0.1.0"
category = "unicorn"
formats = ["vst3"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA-ARCH");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA-ARCH error for unknown category; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 6 — Architecture schema: `[plugin].version` enforces strict SemVer.
//     `0.1.0` validates; `0.1.0-rc1` fails.
// ---------------------------------------------------------------------------
#[test]
fn architecture_schema_rejects_non_strict_semver_version() {
    let (target, _) = fresh_plugin("bad-version");
    write_arch(
        &target,
        r#"[plugin]
name = "Demo"
vendor = "ExampleCo"
version = "0.1.0-rc1"
category = "instrument"
formats = ["vst3"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "SCHEMA-ARCH");
    assert!(
        !errs.is_empty(),
        "expected SCHEMA-ARCH error for non-strict-SemVer version; got {report:#?}"
    );
}

// ---------------------------------------------------------------------------
// 7 — Cargo.toml render: cdylib only when standalone is NOT in formats.
// ---------------------------------------------------------------------------
#[test]
fn cargo_toml_emits_cdylib_only_when_no_standalone() {
    let (target, _) = fresh_plugin("cdylib-only");
    write_arch(&target, ARCH_DEFAULT);

    let layout = LayoutContext::discover(&target, None).unwrap();
    let rendered = render(&layout).unwrap();
    let t = &rendered.text;

    assert!(
        t.contains("[lib]\ncrate-type = [\"cdylib\"]\n"),
        "expected [lib] cdylib block:\n{t}"
    );
    assert!(
        !t.contains("[[bin]]"),
        "expected no [[bin]] target when formats lacks standalone:\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 8 — Cargo.toml render: formats includes "standalone" → emit
//     [[bin]] standalone target with name = "<project>-standalone",
//     path = src/bin/standalone.rs. cdylib still emits.
// ---------------------------------------------------------------------------
#[test]
fn cargo_toml_emits_bin_when_standalone_in_formats() {
    let (target, _) = fresh_plugin("with-standalone");
    write_arch(&target, ARCH_WITH_STANDALONE);

    let layout = LayoutContext::discover(&target, None).unwrap();
    let rendered = render(&layout).unwrap();
    let t = &rendered.text;

    assert!(
        t.contains("[lib]\ncrate-type = [\"cdylib\"]\n"),
        "cdylib still required:\n{t}"
    );
    assert!(
        t.contains("[[bin]]\nname = \"demo_plugin-standalone\"\npath = \"src/bin/standalone.rs\"\n"),
        "expected [[bin]] standalone target:\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 9 — Cargo.toml render: [features] parity with library-api. Architecture-
//     declared features emit a [features] section with synthetic
//     `default = [...]` line.
// ---------------------------------------------------------------------------
#[test]
fn cargo_toml_emits_features_section_with_default_synthesis() {
    let (target, _) = fresh_plugin("features");
    write_arch(
        &target,
        r#"[plugin]
name = "Demo"
vendor = "ExampleCo"
version = "0.1.0"
category = "fx"
formats = ["vst3", "clap"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"

[[features]]
name = "midi"
description = "Optional MIDI handling"

[[features]]
name = "simd"
description = "SIMD DSP path"
default = true
"#,
    );

    let layout = LayoutContext::discover(&target, None).unwrap();
    let rendered = render(&layout).unwrap();
    let t = &rendered.text;

    assert!(t.contains("[features]\n"), "{t}");
    assert!(t.contains("default = [\"simd\"]\n"), "{t}");
    assert!(t.contains("midi = []\n"), "{t}");
    assert!(t.contains("simd = []\n"), "{t}");
}

// ---------------------------------------------------------------------------
// 10 — F-006 parity: requirements_have_satisfiers fires on orphan
//      module-nature requirements; architecture-nature requirements are
//      exempt.
// ---------------------------------------------------------------------------
#[test]
fn requirements_have_satisfiers_fires_on_orphan_module_requirement() {
    let (target, _) = fresh_plugin("orphan-req");
    write_arch(&target, ARCH_DEFAULT);
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-AUDIO-001"
text = "Plugin must not allocate on the audio thread."

[[requirement]]
id = "REQ-FORMAT-001"
text = "Plugin ships as VST3 + CLAP."
nature = "architecture"
"#,
    )
    .unwrap();
    write_module(
        &target,
        "MyPlugin",
        r#"name = "MyPlugin"
layer = "plugin_shell"
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let warnings = warnings_for(&report, "requirements_have_satisfiers");
    assert_eq!(
        warnings.len(),
        1,
        "expected exactly one orphan requirement warning (architecture-nature exempt); got {warnings:#?}"
    );
    let msg = warnings[0].message.clone();
    assert!(
        msg.contains("REQ-AUDIO-001"),
        "warning should name REQ-AUDIO-001: {msg}"
    );
    assert!(
        !msg.contains("REQ-FORMAT-001"),
        "architecture-nature must be exempt: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 11 — feature_gates round-trip: module declaring feature_gates that
//      reference declared features validates clean; references to
//      undeclared features fire REDUCE_UNRESOLVED (the reducer's
//      pre-graph-check unresolved-endpoint detector catches missing
//      features before the `module_feature_gates_resolve` graph check
//      runs — same behavior as library-api).
// ---------------------------------------------------------------------------
#[test]
fn feature_gates_resolve_against_architecture_features() {
    let (target, _) = fresh_plugin("feature-gates");
    write_arch(
        &target,
        r#"[plugin]
name = "Demo"
vendor = "ExampleCo"
version = "0.1.0"
category = "instrument"
formats = ["vst3"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"

[[features]]
name = "midi"
description = "MIDI handling"
"#,
    );
    write_module(
        &target,
        "MidiHandler",
        r#"name = "MidiHandler"
layer = "engine"
feature_gates = ["midi"]
"#,
    );
    write_module(
        &target,
        "BadGate",
        r#"name = "BadGate"
layer = "engine"
feature_gates = ["nonexistent"]
"#,
    );

    let report = validate::run_project(&target, None).unwrap();
    let errs = errors_for(&report, "REDUCE_UNRESOLVED");
    let feature_errs: Vec<_> = errs
        .iter()
        .filter(|f| f.message.contains("Feature") && f.message.contains("nonexistent"))
        .collect();
    assert_eq!(
        feature_errs.len(),
        1,
        "expected one REDUCE_UNRESOLVED error naming Feature/nonexistent; got {errs:#?}"
    );
}
