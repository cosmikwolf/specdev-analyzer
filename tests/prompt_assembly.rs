//! Deterministic prompt assembly.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
    prompt::{detect_relevance, list_module_names},
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
        "specdev-prompt-assembly-{tag}-{}-{}",
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

fn write_uart_module(target: &Path) {
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
}

// ---------------------------------------------------------------------------
// 3 — Module references one block; relevance picks just that block.
// ---------------------------------------------------------------------------
#[test]
fn module_with_one_block_detected() {
    let (target, layout) = fresh_firmware_project("one-block");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Async.toml"),
        r#"name = "Async"
layer = "application"

[[embassy.tasks]]
name = "tick"
pool_size = 1
"#,
    )
    .unwrap();
    let rel = detect_relevance(&layout, "Async").unwrap();
    assert_eq!(rel.blocks, vec!["embassy".to_string()]);
    assert!(rel.bridges.is_empty());
}

// ---------------------------------------------------------------------------
// 4 — Module references both blocks; both detected, lex-sorted.
// ---------------------------------------------------------------------------
#[test]
fn module_with_both_blocks_detected_lex_sorted() {
    let (target, layout) = fresh_firmware_project("both-blocks");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Mixed.toml"),
        r#"name = "Mixed"
layer = "driver"

[[stm32h5.peripherals]]
name = "p"
kind = "I2C"
instance = 1

[[embassy.tasks]]
name = "t"
pool_size = 1
"#,
    )
    .unwrap();
    let rel = detect_relevance(&layout, "Mixed").unwrap();
    assert_eq!(
        rel.blocks,
        vec!["embassy".to_string(), "stm32h5-rs".to_string()],
        "blocks must be lex-sorted (embassy < stm32h5-rs)"
    );
}

// ---------------------------------------------------------------------------
// 5 — Module with bridge bindings → bridge detected.
// ---------------------------------------------------------------------------
#[test]
fn module_with_bridge_bindings_detected() {
    let (target, layout) = fresh_firmware_project("bridge-mod");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target);
    let rel = detect_relevance(&layout, "Uart").unwrap();
    assert_eq!(
        rel.blocks,
        vec!["embassy".to_string(), "stm32h5-rs".to_string()]
    );
    assert_eq!(rel.bridges, vec!["embassy+stm32h5-rs".to_string()]);
}

// A second module that references only the embassy block (no stm32h5,
// no bridge) — used to exercise the preamble's block/bridge union.
fn write_embassy_only_module(target: &Path) {
    std::fs::write(
        target.join("specs/modules/Worker.toml"),
        r#"name = "Worker"
layer = "app"

[[embassy.tasks]]
name = "worker_task"
pool_size = 1
"#,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// F-006 — preamble/tail split.
// ---------------------------------------------------------------------------

// 14 — list_module_names is lex-sorted; empty when no modules dir content.
#[test]
fn list_module_names_lex_sorted() {
    let (target, layout) = fresh_firmware_project("list");
    write_uart_module(&target);
    write_embassy_only_module(&target);
    let names = list_module_names(&layout).unwrap();
    assert_eq!(names, vec!["Uart".to_string(), "Worker".to_string()]);
}

// ---------------------------------------------------------------------------
// F-006 — CLI surface (`gen --preamble` / `gen --module <N> --tail`).
// ---------------------------------------------------------------------------
