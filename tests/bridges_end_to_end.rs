//! First canonical bridge (`embassy+stm32h5-rs`) plus project-local bridge
//! end-to-end.
//!
//! Covers:
//!
//! 1. `specdev inline-bridge <a>+<b>` succeeds only when both blocks are inlined.
//! 2. A module using both blocks plus the canonical bridge validates.
//! 3. Project-local bridge: hand-authored under `specs/specdev/...`
//!    validates end-to-end; no lineage entry expected.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
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
        "specdev-bridges-end-to-end-{tag}-{}-{}",
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

// ---------------------------------------------------------------------------
// 1 — inline-bridge before blocks → INLINE-003 (preflight rejects).
// ---------------------------------------------------------------------------
#[test]
fn inline_bridge_without_blocks_fails() {
    let (_target, layout) = fresh_firmware_project("bridge-no-blocks");
    let err = run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("INLINE-003"),
        "expected INLINE-003; got: {msg}"
    );
    assert!(
        msg.contains("embassy") || msg.contains("stm32h5-rs"),
        "expected blocker name in message; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// 2 — inline-bridge after both blocks succeeds; populates files + lineage.
// ---------------------------------------------------------------------------
#[test]
fn inline_bridge_after_both_blocks_succeeds() {
    let (target, layout) = fresh_firmware_project("bridge-ok");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();

    assert!(target
        .join("specs/specdev/schemas/inlined/bridges/embassy+stm32h5-rs.schema.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/reductions/bridges/embassy+stm32h5-rs.yaml")
        .is_file());
    // Codegen prose is no longer inlined.
    assert!(!target
        .join("specs/specdev/codegen/rust/bridges/embassy+stm32h5-rs/README.md")
        .exists());

    let lineage = Lineage::load(&layout.specdev_root).unwrap();
    let entry = lineage
        .bridges
        .get("embassy+stm32h5-rs")
        .expect("bridge lineage");
    assert!(entry.content_hash.starts_with("sha256:"));
}

// ---------------------------------------------------------------------------
// 3 — re-inline-bridge refused with INLINE-002.
// ---------------------------------------------------------------------------
#[test]
fn re_inline_bridge_refused() {
    let (_target, layout) = fresh_firmware_project("bridge-redo");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    let err = run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap_err();
    assert!(format!("{err:#}").contains("INLINE-002"));
}

// ---------------------------------------------------------------------------
// 4 — module using both blocks + bridge bindings validates.
// ---------------------------------------------------------------------------
#[test]
fn module_with_bridge_bindings_validates() {
    let (target, layout) = fresh_firmware_project("bridge-mod-ok");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Uart.toml"),
        r#"name = "Uart"
layer = "driver"

[[stm32h5.peripherals]]
name = "uart_dev"
kind = "USART"
instance = 3

[[embassy.tasks]]
name = "read_uart"
pool_size = 1

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "read_uart"
peripheral = "uart_dev"
handoff = "owned"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 5 — invalid handoff enum value rejected.
// ---------------------------------------------------------------------------
#[test]
fn module_with_invalid_handoff_rejects() {
    let (target, layout) = fresh_firmware_project("bridge-bad-handoff");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Bad.toml"),
        r#"name = "Bad"
layer = "driver"

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "x"
peripheral = "y"
handoff = "interrupt_driven"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"SCHEMA".to_string()),
        "expected SCHEMA finding; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 6 — bindings without peripheral_tasks (empty / omitted) still validates.
// ---------------------------------------------------------------------------
#[test]
fn module_without_bridge_bindings_validates() {
    let (target, layout) = fresh_firmware_project("bridge-mod-empty");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    // Module declares both blocks but no [bindings."embassy+stm32h5-rs"]
    // — bridge inlined, but module uses no bindings. Should validate.
    std::fs::write(
        target.join("specs/modules/Independent.toml"),
        r#"name = "Independent"
layer = "driver"

[[stm32h5.peripherals]]
name = "led_pin_bank"
kind = "GPIO"
instance = 2

[[embassy.tasks]]
name = "blink"
pool_size = 1
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 7 — Project-local bridge end-to-end: author a local bridge for a
//     fictional pair, validate the module, confirm no lineage entry.
// ---------------------------------------------------------------------------
#[test]
fn project_local_bridge_validates_without_lineage() {
    let (target, layout) = fresh_firmware_project("local-bridge");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    // No `inline-bridge` for this fictional pair — author the local
    // bridge files directly. Path: project-local bridges live under
    // `reductions/bridges/local/` (not the canonical `bridges/`
    // subdirectory).

    // Local bridge schema fragment — adds shape under
    // bindings."embassy+local-thing" on Module.
    let schemas_dir = target.join("specs/specdev/schemas/inlined/bridges");
    std::fs::create_dir_all(&schemas_dir).unwrap();
    std::fs::write(
        schemas_dir.join("embassy+local-thing.schema.yaml"),
        r#"$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "https://specdev.dev/bridges/embassy+local-thing.v1"
type: object
properties:
  bindings:
    type: object
    properties:
      "embassy+local-thing":
        type: object
        additionalProperties: false
        properties:
          notes:
            type: array
            items: { type: string }
"#,
    )
    .unwrap();

    // Append $ref to module.schema.yaml's allOf so the bindings
    // contribution participates in module validation.
    let module_schema_path = layout.module_schema_path.clone();
    let mut module_schema_text = std::fs::read_to_string(&module_schema_path).unwrap();
    module_schema_text.push_str("- $ref: inlined/bridges/embassy+local-thing.schema.yaml\n");
    std::fs::write(&module_schema_path, module_schema_text).unwrap();

    // Local bridge reduction. Lives under reductions/bridges/local/;
    // analyzer's `reductions::load_all` walks this directory and merges
    // its entries with canonical bridges.
    let local_red_dir = target.join("specs/specdev/reductions/bridges/local");
    std::fs::create_dir_all(&local_red_dir).unwrap();
    std::fs::write(
        local_red_dir.join("embassy+local-thing.yaml"),
        r#"metaschema_version: "v1"

# Project-local bridge between embassy tasks and an imagined external
# block ("local-thing"). v1 declares no relations — the bindings are
# annotative-only at this stage.
entities: []
relations: []
"#,
    )
    .unwrap();

    // Module using the local bridge's bindings shape.
    std::fs::write(
        target.join("specs/modules/Mixed.toml"),
        r#"name = "Mixed"
layer = "driver"

[[embassy.tasks]]
name = "main_loop"
pool_size = 1

[bindings."embassy+local-thing"]
notes = ["wired in the project's local bridge"]
"#,
    )
    .unwrap();

    let report = validate::run_project(&target, None).unwrap();
    assert!(
        errors(&report).is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );

    // No lineage entry — local bridges don't get one by design.
    let lineage = Lineage::load(&layout.specdev_root).unwrap();
    assert!(
        !lineage.has_bridge("embassy+local-thing"),
        "local bridges must not appear in lineage.toml"
    );
}
