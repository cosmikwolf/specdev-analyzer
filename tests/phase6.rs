//! Phase-6 integration tests: first concrete blocks (embassy + stm32h5-rs).
//!
//! Exit criteria from `specdev-next/05-implementation-plan.md` Phase 6:
//!
//! 1. `specdev inline <name>` populates every expected file + lineage.
//! 2. A module TOML using the inlined block validates first try.
//! 3. Each block independently passes the doc 15 §3.1 sharp-test
//!    review (manual checklist; not automated here).

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::run_inline_block,
    layout::LayoutContext,
    lineage::Lineage,
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
        "specdev-phase6-{tag}-{}-{}",
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
        &phase6_plugin(),
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
// 1 — inline embassy populates files + lineage
// ---------------------------------------------------------------------------
#[test]
fn inline_embassy_populates_files_and_lineage() {
    let (target, layout) = fresh_firmware_project("inline-embassy");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();

    assert!(target
        .join("specs/specdev/schemas/inlined/blocks/embassy.schema.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/reductions/blocks/embassy.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/codegen/rust/blocks/embassy/README.md")
        .is_file());

    let lineage = Lineage::load(&layout.specdev_root).unwrap();
    let entry = lineage.blocks.get("embassy").expect("embassy lineage");
    assert!(entry.content_hash.starts_with("sha256:"));
}

// ---------------------------------------------------------------------------
// 2 — inline stm32h5-rs populates files + lineage
// ---------------------------------------------------------------------------
#[test]
fn inline_stm32h5_rs_populates_files_and_lineage() {
    let (target, layout) = fresh_firmware_project("inline-stm32h5");
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();

    assert!(target
        .join("specs/specdev/schemas/inlined/blocks/stm32h5-rs.schema.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/reductions/blocks/stm32h5-rs.yaml")
        .is_file());
    assert!(target
        .join("specs/specdev/codegen/rust/blocks/stm32h5-rs/README.md")
        .is_file());

    let lineage = Lineage::load(&layout.specdev_root).unwrap();
    assert!(lineage.blocks.contains_key("stm32h5-rs"));
}

// ---------------------------------------------------------------------------
// 3 — module using embassy validates first try
// ---------------------------------------------------------------------------
#[test]
fn module_using_embassy_validates() {
    let (target, layout) = fresh_firmware_project("mod-embassy");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    std::fs::write(
        target.join("specs/modules/Audio.toml"),
        r#"name = "Audio"
layer = "driver"
description = "Audio driver."

[[embassy.tasks]]
name = "capture"
pool_size = 1

[[embassy.tasks]]
name = "playback"
priority = "high"

[[embassy.channels]]
name = "samples"
payload_type = "[u8; 64]"
depth = 8
kind = "bounded"
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
// 4 — module using stm32h5-rs validates first try
// ---------------------------------------------------------------------------
#[test]
fn module_using_stm32h5_rs_validates() {
    let (target, layout) = fresh_firmware_project("mod-stm32");
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Uart.toml"),
        r#"name = "Uart"
layer = "driver"

[[stm32h5.peripherals]]
name = "uart_dev"
kind = "USART"
instance = 1

[[stm32h5.peripherals]]
name = "gpio_a"
kind = "GPIO"
instance = 1

[[stm32h5.pins]]
name = "tx"
port = "A"
pin = 9
alt_fn = 7
mode = "alternate"
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
// 5 — module using both blocks validates
// ---------------------------------------------------------------------------
#[test]
fn module_using_both_blocks_validates() {
    let (target, layout) = fresh_firmware_project("mod-both");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Sensor.toml"),
        r#"name = "Sensor"
layer = "driver"

[[stm32h5.peripherals]]
name = "spi_dev"
kind = "SPI"
instance = 1

[[stm32h5.pins]]
name = "sck"
port = "B"
pin = 3
alt_fn = 5
mode = "alternate"

[[embassy.tasks]]
name = "poll"
pool_size = 1

[[embassy.channels]]
name = "readings"
payload_type = "f32"
depth = 4
kind = "watch"
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
// 6 — embassy schema rejects unknown field
// ---------------------------------------------------------------------------
#[test]
fn embassy_schema_rejects_unknown_field() {
    let (target, layout) = fresh_firmware_project("emb-bad-field");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    std::fs::write(
        target.join("specs/modules/Bad.toml"),
        r#"name = "Bad"
layer = "driver"

[[embassy.tasks]]
name = "x"
unknown_field = "should reject"
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
// 7 — stm32h5-rs schema rejects invalid kind enum value
// ---------------------------------------------------------------------------
#[test]
fn stm32h5_schema_rejects_invalid_kind() {
    let (target, layout) = fresh_firmware_project("stm32-bad-kind");
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Bad.toml"),
        r#"name = "Bad"
layer = "driver"

[[stm32h5.peripherals]]
name = "x"
kind = "INVALID_KIND"
instance = 1
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    assert!(
        rule_codes(&report).contains(&"SCHEMA".to_string()),
        "expected SCHEMA finding for invalid enum; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 8 — `[embassy]` table without inlining the block is rejected
// ---------------------------------------------------------------------------
#[test]
fn embassy_table_without_inline_rejects() {
    let (target, _layout) = fresh_firmware_project("emb-no-inline");
    // Author module with [embassy] but DO NOT inline the embassy block.
    std::fs::write(
        target.join("specs/modules/Mod.toml"),
        r#"name = "Mod"
layer = "driver"

[[embassy.tasks]]
name = "x"
"#,
    )
    .unwrap();
    let report = validate::run_project(&target, None).unwrap();
    // The aggregator hasn't added embassy's $ref to allOf, so `embassy` is
    // unevaluated → unevaluatedProperties rejection.
    assert!(
        rule_codes(&report).contains(&"SCHEMA".to_string()),
        "expected SCHEMA rejection for unevaluated property; got: {:?}\n{}",
        rule_codes(&report),
        report.render_plain()
    );
}

// ---------------------------------------------------------------------------
// 9 — embassy.Task entities materialize with qualified names
// ---------------------------------------------------------------------------
#[test]
fn embassy_task_materializes_qualified() {
    use specdev_analyzer::reduction;

    let (target, layout) = fresh_firmware_project("emb-extract");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    std::fs::write(
        target.join("specs/modules/Audio.toml"),
        r#"name = "Audio"
layer = "driver"

[[embassy.tasks]]
name = "capture"

[[embassy.channels]]
name = "samples"
payload_type = "u8"
depth = 4
"#,
    )
    .unwrap();
    let (graph, _report) = reduction::run_project(&target, None).unwrap();
    let task_names: Vec<&str> = graph
        .entities
        .iter()
        .filter(|e| e.kind == "embassy.Task")
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(task_names, vec!["Audio.capture"]);
    let channel_names: Vec<&str> = graph
        .entities
        .iter()
        .filter(|e| e.kind == "embassy.Channel")
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(channel_names, vec!["Audio.samples"]);
}

// ---------------------------------------------------------------------------
// 10 — stm32h5-rs.Peripheral entities materialize with qualified names
// ---------------------------------------------------------------------------
#[test]
fn stm32h5_peripheral_materializes_qualified() {
    use specdev_analyzer::reduction;

    let (target, layout) = fresh_firmware_project("stm32-extract");
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Uart.toml"),
        r#"name = "Uart"
layer = "driver"

[[stm32h5.peripherals]]
name = "uart_dev"
kind = "USART"
instance = 1

[[stm32h5.pins]]
name = "tx"
port = "A"
pin = 9
mode = "alternate"
"#,
    )
    .unwrap();
    let (graph, _report) = reduction::run_project(&target, None).unwrap();
    let periph_names: Vec<&str> = graph
        .entities
        .iter()
        .filter(|e| e.kind == "stm32h5-rs.Peripheral")
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(periph_names, vec!["Uart.uart_dev"]);
    let pin_names: Vec<&str> = graph
        .entities
        .iter()
        .filter(|e| e.kind == "stm32h5-rs.Pin")
        .map(|e| e.name.as_str())
        .collect();
    assert_eq!(pin_names, vec!["Uart.tx"]);
}
