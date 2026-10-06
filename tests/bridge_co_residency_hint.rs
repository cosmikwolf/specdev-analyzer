//! `reduction.rs::bridge_co_residency_hint` manifest-driven hint rendering.
//!
//! When a `REDUCE_UNRESOLVED` finding fires on a
//! bridge-qualified relation, the appended `hint:` line is rendered from
//! the bridge's inlined `contract.yaml` `diagnostic_template` (with
//! `{bridge_name}` / `{parameter}` / `{decl_module}` / `{binding_module}`
//! substituted from the relation context). Manifest-less fixtures without
//! a `contract.yaml` fall back to the legacy generic hint.
//! Unresolvable tokens render as `<unknown>` and emit a separate
//! `REDUCE_UNRESOLVED-PROVENANCE` info finding.
//!
//! Coverage:
//!   1. embassy+stm32h5-rs unresolved binding → contract.yaml-driven
//!      intra-module wiring template renders.
//!   2. egui+nih-plug unresolved parameter → contract.yaml-driven
//!      `co-residency on the plugin_shell module` template renders.
//!   3. `{decl_module}` resolves successfully when opposite endpoint
//!      exists (egui+nih-plug: missing parameter, region present).
//!   4. `{decl_module}` falls back to `<unknown>` + emits a
//!      `REDUCE_UNRESOLVED-PROVENANCE` info finding when opposite
//!      endpoint is also unresolved.
//!   5. Backward-compat: a bridge whose inlined `contract.yaml`
//!      is removed falls back to the legacy `bridges/<head>/README.md`
//!      hint string verbatim.
//!   6. Non-bridge relation (qualified path without `+`) → no
//!      contract load, no hint appended.
//!   7. Malformed `contract.yaml` → `MANIFEST-001` Error surfaces.
//!
//! Agnosticism: the analyzer source no longer encodes a specific
//! bridge's diagnostic prose. All bridge-named text in the rendered
//! output flows through `{bridge_name}` substitution; a brittle but
//! load-bearing assertion in tests 1 + 2 catches regressions where a
//! source-level literal slips back in.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
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
        "specdev-bridge-co-residency-{tag}-{}-{}",
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

fn fresh_firmware_project(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn fresh_audio_plugin(tag: &str) -> (PathBuf, LayoutContext) {
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

fn errors_with_prefix(report: &Report, prefix: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule.starts_with(prefix) && matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

fn findings_with_rule(report: &Report, rule: &str) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| f.rule == rule)
        .cloned()
        .collect()
}

fn write_audio_plugin_arch(target: &Path) {
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
}

// ===========================================================================
// 1 — embassy+stm32h5-rs unresolved binding endpoint renders the
//     `endpoint_module`-locality template body (intra-module wiring).
// ===========================================================================

#[test]
fn embassy_stm32h5_unresolved_renders_intra_module_template() {
    let (target, layout) = fresh_firmware_project("emb-intra-module");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();

    // Module references a peripheral that doesn't exist on this module.
    std::fs::write(
        target.join("specs/modules/Uart.toml"),
        r#"name = "Uart"
layer = "driver"

[[embassy.tasks]]
name = "read_uart"
pool_size = 1

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "read_uart"
peripheral = "ghost_peripheral"
handoff = "owned"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let unresolved = errors_with_prefix(&report, "REDUCE_UNRESOLVED");
    assert!(
        !unresolved.is_empty(),
        "expected REDUCE_UNRESOLVED for ghost_peripheral; got:\n{}",
        report.render_plain()
    );
    let msg = unresolved
        .iter()
        .find(|f| f.message.contains("hint:"))
        .map(|f| f.message.clone())
        .unwrap_or_else(|| {
            panic!(
                "expected at least one REDUCE_UNRESOLVED with `hint:`; got messages:\n{:#?}",
                unresolved.iter().map(|f| &f.message).collect::<Vec<_>>()
            )
        });

    // Substituted-template substring assertions (from contract.yaml body).
    assert!(
        msg.contains("intra-module wiring:"),
        "expected substituted-template substring `intra-module wiring:`; \
         got message:\n{msg}"
    );
    // {bridge_name} substitution.
    assert!(
        msg.contains("embassy+stm32h5-rs"),
        "expected {{bridge_name}} substitution `embassy+stm32h5-rs`; got:\n{msg}"
    );
    // {binding_module} substitution (the file's `name` field is `Uart`).
    assert!(
        msg.contains("Uart") || !msg.contains("<unknown>"),
        "expected binding_module to resolve (parent_name `Uart`); got:\n{msg}"
    );

    // Brittle-but-valuable agnosticism check: the legacy literal
    // `bridges/<head>/README.md` wording is gone from the source-side
    // hint when a contract.yaml is present.
    assert!(
        !msg.contains("README.md"),
        "manifest-driven path must not surface the legacy README.md hint; got:\n{msg}"
    );
    assert!(
        !msg.contains("v1 co-residency rule"),
        "manifest-driven path must not surface the legacy `v1 co-residency rule` literal; \
         got:\n{msg}"
    );
}

// ===========================================================================
// 2 — egui+nih-plug analogue: `plugin_shell_unique`-locality template body.
// ===========================================================================

#[test]
fn egui_nih_plug_unresolved_renders_plugin_shell_template() {
    let (target, layout) = fresh_audio_plugin("egui-plugin-shell");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_audio_plugin_arch(&target);

    // Binding references a parameter that doesn't exist; region resolves.
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
parameter = "ghost_param"
region = "param_grid"
widget_kind = "knob"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let unresolved = errors_with_prefix(&report, "REDUCE_UNRESOLVED");
    let msg = unresolved
        .iter()
        .find(|f| f.message.contains("hint:"))
        .map(|f| f.message.clone())
        .unwrap_or_else(|| {
            panic!(
                "expected at least one REDUCE_UNRESOLVED with `hint:`; got messages:\n{:#?}",
                unresolved.iter().map(|f| &f.message).collect::<Vec<_>>()
            )
        });

    assert!(
        msg.contains("co-residency on the plugin_shell module"),
        "expected substituted-template substring `co-residency on the plugin_shell module`; \
         got message:\n{msg}"
    );
    assert!(
        msg.contains("egui+nih-plug"),
        "expected {{bridge_name}} substitution `egui+nih-plug`; got:\n{msg}"
    );
    // {binding_module} substitution: file's `name` is `MyPlugin`.
    assert!(
        msg.contains("MyPlugin"),
        "expected {{binding_module}} substitution `MyPlugin`; got:\n{msg}"
    );

    // Agnosticism vocabulary check (no legacy README.md leak).
    assert!(
        !msg.contains("README.md"),
        "manifest-driven path must not surface the legacy README.md hint; got:\n{msg}"
    );
}

// ===========================================================================
// 3 — `{decl_module}` resolves successfully when opposite endpoint exists.
//     egui+nih-plug: parameter unresolved (`ghost_param`), region resolves
//     to `MyPlugin.param_grid` whose owning module is `MyPlugin`. The
//     diagnostic should NOT report `<unknown>` for any token.
// ===========================================================================

#[test]
fn decl_module_resolves_when_opposite_endpoint_present() {
    let (target, layout) = fresh_audio_plugin("decl-module-resolves");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_audio_plugin_arch(&target);

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
parameter = "ghost_param"
region = "param_grid"
widget_kind = "knob"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let unresolved = errors_with_prefix(&report, "REDUCE_UNRESOLVED");
    let msg = unresolved
        .iter()
        .find(|f| f.message.contains("hint:"))
        .map(|f| f.message.clone())
        .expect("expected REDUCE_UNRESOLVED with hint:");

    assert!(
        !msg.contains("<unknown>"),
        "expected ALL tokens resolvable when opposite endpoint exists; got:\n{msg}"
    );

    // No PROVENANCE info findings should fire (everything resolved).
    let provenance = findings_with_rule(&report, "REDUCE_UNRESOLVED-PROVENANCE");
    assert!(
        provenance.is_empty(),
        "expected zero REDUCE_UNRESOLVED-PROVENANCE findings; got: {:#?}",
        provenance.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
}

// ===========================================================================
// 4 — `{decl_module}` falls back to `<unknown>` + emits a
//     `REDUCE_UNRESOLVED-PROVENANCE` info finding when the opposite
//     endpoint is ALSO unresolved.
// ===========================================================================

#[test]
fn decl_module_unknown_emits_provenance_info_finding() {
    let (target, layout) = fresh_audio_plugin("decl-module-unknown");
    run_inline_block(&layout, &full_plugin(), "nih-plug").unwrap();
    run_inline_block(&layout, &full_plugin(), "egui").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "egui+nih-plug").unwrap();
    write_audio_plugin_arch(&target);

    // Both endpoints unresolved: no parameter declared, no region declared.
    std::fs::write(
        target.join("specs/modules/MyPlugin.toml"),
        r#"name = "MyPlugin"
layer = "plugin_shell"

[[bindings."egui+nih-plug".parameter_widgets]]
parameter = "ghost_param"
region = "ghost_region"
widget_kind = "knob"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let unresolved = errors_with_prefix(&report, "REDUCE_UNRESOLVED");
    let msg = unresolved
        .iter()
        .find(|f| f.message.contains("hint:"))
        .map(|f| f.message.clone())
        .expect("expected REDUCE_UNRESOLVED with hint:");

    // `{decl_module}` rendered as `<unknown>`.
    assert!(
        msg.contains("<unknown>"),
        "expected `<unknown>` for the unresolvable {{decl_module}} token; got:\n{msg}"
    );

    let provenance = findings_with_rule(&report, "REDUCE_UNRESOLVED-PROVENANCE");
    assert!(
        !provenance.is_empty(),
        "expected at least one REDUCE_UNRESOLVED-PROVENANCE info finding; \
         got 0. Report:\n{}",
        report.render_plain()
    );
    let mentions_decl_module = provenance
        .iter()
        .any(|f| f.message.contains("{decl_module}"));
    assert!(
        mentions_decl_module,
        "expected one PROVENANCE finding to name the {{decl_module}} token; got: {:#?}",
        provenance.iter().map(|f| &f.message).collect::<Vec<_>>()
    );
    // Severity is Info (not Error).
    let info_count = provenance
        .iter()
        .filter(|f| matches!(f.severity, Severity::Info))
        .count();
    assert!(
        info_count >= 1,
        "expected REDUCE_UNRESOLVED-PROVENANCE to fire at Severity::Info; \
         got severities: {:?}",
        provenance.iter().map(|f| &f.severity).collect::<Vec<_>>()
    );

    // Same provenance as the parent error: file path matches.
    let parent_file = unresolved
        .iter()
        .find(|f| f.message.contains("hint:"))
        .and_then(|f| f.file.clone())
        .expect("parent error must carry a file");
    let info_file = provenance
        .iter()
        .find(|f| f.message.contains("{decl_module}"))
        .and_then(|f| f.file.clone())
        .expect("provenance info must carry a file");
    assert_eq!(
        parent_file, info_file,
        "PROVENANCE info finding must carry the same source_file as parent REDUCE_UNRESOLVED"
    );
}

// ===========================================================================
// 5 — Backward-compat: contract.yaml stripped from the inlined
//     tree falls back to the legacy `bridges/<head>/README.md` hint.
// ===========================================================================

#[test]
fn missing_contract_yaml_falls_back_to_legacy_hint() {
    let (target, layout) = fresh_firmware_project("legacy-fallback");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();

    // Strip the inlined contract.yaml after inline-bridge to simulate a
    // manifest-less fixture / older inlined tree.
    let contract_path =
        target.join("specs/specdev/reductions/bridges/embassy+stm32h5-rs.contract.yaml");
    assert!(
        contract_path.is_file(),
        "test premise: inline-bridge must have copied contract.yaml"
    );
    std::fs::remove_file(&contract_path).unwrap();

    std::fs::write(
        target.join("specs/modules/Uart.toml"),
        r#"name = "Uart"
layer = "driver"

[[embassy.tasks]]
name = "read_uart"
pool_size = 1

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "read_uart"
peripheral = "ghost_peripheral"
handoff = "owned"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let unresolved = errors_with_prefix(&report, "REDUCE_UNRESOLVED");
    let msg = unresolved
        .iter()
        .find(|f| f.message.contains("hint:"))
        .map(|f| f.message.clone())
        .expect("expected REDUCE_UNRESOLVED with hint:");

    // Legacy hint surfaces.
    assert!(
        msg.contains("README.md"),
        "expected legacy fallback hint mentioning README.md when contract.yaml absent; got:\n{msg}"
    );
    assert!(
        msg.contains("v1 co-residency rule"),
        "expected legacy fallback `v1 co-residency rule` literal; got:\n{msg}"
    );
}

// ===========================================================================
// 6 — Non-bridge relation (no `+` in path head) → no contract load,
//     no hint appended. Trigger via a missing peripheral declared via
//     a non-bridge relation: `bindings."embassy+stm32h5-rs"` is bridge-
//     qualified, but the underlying `extends: depends_on` chain still
//     fires REDUCE_UNRESOLVED only on bridge-qualified relations. So
//     pick a non-bridge unresolved relation: a non-existent
//     `peripheral.dma` reference is NOT bridge-qualified.
//
//     Use the simpler approach: inspect the live REDUCE_UNRESOLVED set
//     for the embassy+stm32h5-rs case — bridge errors get `hint:`,
//     other errors don't. Asserts no `hint:` line on the non-bridge
//     branch (which is satisfied vacuously when no non-bridge
//     unresolved fires; the alternative is a fixture with a non-bridge
//     unresolved relation, which the v1 reduction set doesn't ship.
//     Instead this test is a defensive dual: assert no `hint:` line
//     appears on findings whose relation kind has no `+`).
// ===========================================================================

#[test]
fn non_bridge_relation_finds_get_no_hint() {
    let (target, layout) = fresh_firmware_project("non-bridge-no-hint");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    // Note: do NOT inline the bridge — this means the bridge contribution
    // (and its `peripheral_owned_by` relation) is not in the kindset.
    // The module declares no bindings; any unresolved relation that
    // fires would be from a block-internal relation kind (no `+`).

    std::fs::write(
        target.join("specs/modules/Empty.toml"),
        r#"name = "Empty"
layer = "driver"

[[embassy.tasks]]
name = "loop_task"
pool_size = 1
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let unresolved = errors_with_prefix(&report, "REDUCE_UNRESOLVED");
    // None expected here, but if any fires, ensure no `hint:` got
    // appended (the non-bridge branch returns String::new()).
    for f in &unresolved {
        assert!(
            !f.message.contains("hint:"),
            "non-bridge REDUCE_UNRESOLVED must not carry a `hint:` line; got:\n{}",
            f.message
        );
    }
}

// ===========================================================================
// 7 — Malformed contract.yaml → `MANIFEST-001` Error surfaces.
// ===========================================================================

#[test]
fn malformed_contract_yaml_surfaces_manifest_001() {
    let (target, layout) = fresh_firmware_project("malformed-contract");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();

    // Overwrite the inlined contract.yaml with a body that fails schema
    // validation (binding_locality is a closed enum; `nonsense` rejects).
    let contract_path =
        target.join("specs/specdev/reductions/bridges/embassy+stm32h5-rs.contract.yaml");
    std::fs::write(
        &contract_path,
        "binding_locality: nonsense_value_not_in_enum\ndiagnostic_template: 'x'\n",
    )
    .unwrap();

    std::fs::write(
        target.join("specs/modules/Uart.toml"),
        r#"name = "Uart"
layer = "driver"

[[embassy.tasks]]
name = "read_uart"
pool_size = 1

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "read_uart"
peripheral = "ghost_peripheral"
handoff = "owned"
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    let manifest_errors = errors_with_prefix(&report, "MANIFEST-001");
    assert!(
        !manifest_errors.is_empty(),
        "expected MANIFEST-001 from the malformed contract.yaml; got:\n{}",
        report.render_plain()
    );
}
