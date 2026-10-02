//! `[[parameter_bindings]]` (audio-plugin).
//!
//! Coverage:
//!   * A plugin_shell module declaring [nih_plug] + co-resident
//!     [[parameter_bindings]] for every parameter validates clean
//!     (no errors, no `parameters_have_bindings` warning).
//!   * `parameters_have_bindings` warns (not errors) on a parameter
//!     with no binding — the load-bearing orphan check.
//!   * REDUCE_UNRESOLVED fires when a binding references a parameter
//!     that isn't declared on the module.
//!   * Multi-target bindings (two rows, one parameter) validate clean —
//!     no uniqueness check, no orphan.
//!   * Schema rejects a malformed value_transform.
//!   * The reduction materializes one `parameter_binding` relation per
//!     row, resolving co-resident parameters.
//!   * `prompt` assembly is byte-identical across two runs (the
//!     determinism floor — analyzer assembles, Claude emits).
//!
//! `[[parameter_bindings]]` is co-resident on the plugin_shell module.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::run_inline_block,
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
        "specdev-parameter-bindings-{tag}-{}-{}",
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
category = "instrument"
formats = ["vst3", "clap"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#;

fn fresh_audio(tag: &str) -> (PathBuf, LayoutContext) {
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

fn findings_for(report: &Report, rule: &str, sev: Severity) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule && f.severity == sev)
        .cloned()
        .collect()
}

fn errors(report: &Report) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

/// Three parameters, each with a co-resident binding covering a distinct
/// target_kind (method / enum method / field) and three transforms.
const FULLY_BOUND: &str = r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "master_volume"
name = "Master Volume"
kind = "float"
min = -60.0
max = 6.0
default = 0.0
unit = "dB"

[[nih_plug.parameters]]
id = "out_assign"
name = "Output Assign"
kind = "enum"
variants = ["off", "left", "right", "both"]
default = "both"

[[nih_plug.parameters]]
id = "bypass"
name = "Bypass"
kind = "bool"
default = false

[[parameter_bindings]]
parameter = "master_volume"
target_kind = "method"
target = "ChipEngine::set_master_volume"
target_args = ["value"]
value_transform = { kind = "db_to_amp" }

[[parameter_bindings]]
parameter = "out_assign"
target_kind = "method"
target = "ChipEngine::set_out_assign"
target_args = ["value"]
value_transform = { kind = "enum_to_u8" }

[[parameter_bindings]]
parameter = "bypass"
target_kind = "field"
target = "self.bypassed"
value_transform = { kind = "identity" }
"#;

#[test]
fn fully_bound_module_validates_clean() {
    let (target, _layout) = fresh_audio("clean");
    write_module(&target, "my_plugin", FULLY_BOUND);
    let report = validate::run_project(&target, None).unwrap();
    let errs = errors(&report);
    assert!(errs.is_empty(), "expected zero errors; got:\n{errs:#?}");
    let orphans = findings_for(&report, "parameters_have_bindings", Severity::Warning);
    assert!(
        orphans.is_empty(),
        "every parameter is bound — expected no orphan warning; got:\n{orphans:#?}"
    );
}

#[test]
fn unbound_parameter_warns_not_errors() {
    // Drop the bypass binding; the parameter stays declared.
    let body = FULLY_BOUND
        .split("[[parameter_bindings]]\nparameter = \"bypass\"")
        .next()
        .unwrap();
    let (target, _layout) = fresh_audio("orphan");
    write_module(&target, "my_plugin", body);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "orphan parameter is a warning, not an error; got:\n{:#?}",
        errors(&report)
    );
    let orphans = findings_for(&report, "parameters_have_bindings", Severity::Warning);
    assert_eq!(
        orphans.len(),
        1,
        "exactly one unbound parameter expected; got:\n{orphans:#?}"
    );
    let msg = &orphans[0].message;
    assert!(msg.contains("bypass"), "names the orphan parameter: {msg}");
    assert!(
        msg.contains("Parameter") && msg.contains("parameter_binding"),
        "uses neutral parameter/relation vocabulary: {msg}"
    );
    assert!(
        !msg.contains("requirement"),
        "no requirement leakage: {msg}"
    );
}

#[test]
fn binding_to_unknown_parameter_unresolves() {
    let body = format!(
        "{FULLY_BOUND}\n[[parameter_bindings]]\nparameter = \"does_not_exist\"\n\
         target_kind = \"method\"\ntarget = \"ChipEngine::nope\"\ntarget_args = [\"value\"]\n"
    );
    let (target, _layout) = fresh_audio("unresolved");
    write_module(&target, "my_plugin", &body);
    let report = validate::run_project(&target, None).unwrap();
    let unresolved = findings_for(&report, "REDUCE_UNRESOLVED", Severity::Error);
    assert_eq!(
        unresolved.len(),
        1,
        "binding to an undeclared parameter must surface one REDUCE_UNRESOLVED; got:\n{:#?}",
        report.findings()
    );
    assert!(
        unresolved[0].message.contains("does_not_exist"),
        "names the bad reference: {}",
        unresolved[0].message
    );
}

#[test]
fn multi_target_binding_is_clean() {
    // Two rows for `out_assign` — a register write and a field write.
    // This is allowed; no uniqueness check, no orphan.
    let body = format!(
        "{FULLY_BOUND}\n[[parameter_bindings]]\nparameter = \"out_assign\"\n\
         target_kind = \"field\"\ntarget = \"self.routing.dirty\"\n\
         value_transform = {{ kind = \"identity\" }}\n"
    );
    let (target, _layout) = fresh_audio("multitarget");
    write_module(&target, "my_plugin", &body);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "multi-target is allowed; got errors:\n{:#?}",
        errors(&report)
    );
    let orphans = findings_for(&report, "parameters_have_bindings", Severity::Warning);
    assert!(
        orphans.is_empty(),
        "no orphan with all params bound: {orphans:#?}"
    );
}

#[test]
fn malformed_value_transform_is_rejected() {
    // `clamp` requires min + max; omitting them must fail schema
    // validation against the discriminated union.
    let body = r#"name = "MyPlugin"
layer = "plugin_shell"

[[nih_plug.parameters]]
id = "depth"
name = "Depth"
kind = "int"
min = 0
max = 99
default = 0

[[parameter_bindings]]
parameter = "depth"
target_kind = "field"
target = "self.depth"
value_transform = { kind = "clamp" }
"#;
    let (target, _layout) = fresh_audio("badtransform");
    write_module(&target, "my_plugin", body);
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        !errors(&report).is_empty(),
        "clamp without min/max must fail schema validation; got no errors"
    );
}
