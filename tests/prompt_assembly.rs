//! Deterministic prompt assembly.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
    prompt::{assemble_module_prompt, detect_relevance},
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
// 1 — Module not found → error.
// ---------------------------------------------------------------------------
#[test]
fn missing_module_errors() {
    let (_target, layout) = fresh_firmware_project("missing-mod");
    let err = assemble_module_prompt(&layout, "Nope").unwrap_err();
    assert!(format!("{err:#}").contains("Nope"));
}

// ---------------------------------------------------------------------------
// 2 — Module with no blocks/bridges (only profile-shaped fields) emits a
//     profile-only prompt, no block/bridge sections.
// ---------------------------------------------------------------------------
#[test]
fn module_without_blocks_emits_profile_only() {
    let (target, layout) = fresh_firmware_project("plain-mod");
    std::fs::write(
        target.join("specs/modules/Plain.toml"),
        r#"name = "Plain"
layer = "application"
"#,
    )
    .unwrap();
    let rel = detect_relevance(&layout, "Plain").unwrap();
    assert!(rel.blocks.is_empty(), "no blocks expected; got {:?}", rel.blocks);
    assert!(rel.bridges.is_empty(), "no bridges expected; got {:?}", rel.bridges);

    let prompt = assemble_module_prompt(&layout, "Plain").unwrap();
    assert!(prompt.contains("## Profile module conventions"));
    assert!(!prompt.contains("## Block:"));
    assert!(!prompt.contains("## Bridge:"));
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

// ---------------------------------------------------------------------------
// 6 — Byte-stable across three runs (the load-bearing property: the
//     assembled prompt string must be byte-stable across calls).
// ---------------------------------------------------------------------------
#[test]
fn assembled_prompt_byte_stable_across_runs() {
    let (target, layout) = fresh_firmware_project("stable");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target);
    let a = assemble_module_prompt(&layout, "Uart").unwrap();
    let b = assemble_module_prompt(&layout, "Uart").unwrap();
    let c = assemble_module_prompt(&layout, "Uart").unwrap();
    assert_eq!(a, b, "prompt must be byte-stable across calls");
    assert_eq!(b, c);
}

// ---------------------------------------------------------------------------
// 7 — Assembled prompt contains every expected section in order.
// ---------------------------------------------------------------------------
#[test]
fn assembled_prompt_has_expected_sections_in_order() {
    let (target, layout) = fresh_firmware_project("sections");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target);
    let prompt = assemble_module_prompt(&layout, "Uart").unwrap();

    let header_positions: Vec<usize> = [
        "## Profile module conventions",
        "## Block: `embassy`",
        "## Block: `stm32h5-rs`",
        "## Bridge: `embassy+stm32h5-rs`",
        "## Architecture (`specs/architecture.toml`)",
        "## Module spec (`specs/modules/Uart.toml`)",
    ]
    .iter()
    .map(|h| prompt.find(h).unwrap_or_else(|| panic!("missing header: {h}")))
    .collect();

    let sorted = {
        let mut s = header_positions.clone();
        s.sort();
        s
    };
    assert_eq!(
        header_positions, sorted,
        "headers must appear in fixed order"
    );
}

// ---------------------------------------------------------------------------
// 8 — Architecture section TOML-fenced; module section TOML-fenced.
// ---------------------------------------------------------------------------
#[test]
fn architecture_and_module_are_toml_fenced() {
    let (target, layout) = fresh_firmware_project("fenced");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    write_uart_module(&target);
    let prompt = assemble_module_prompt(&layout, "Uart").unwrap();

    let arch_idx = prompt
        .find("## Architecture")
        .expect("architecture header present");
    let arch_section = &prompt[arch_idx..];
    assert!(
        arch_section.contains("```toml"),
        "architecture section should be TOML-fenced"
    );

    let mod_idx = prompt
        .find("## Module spec")
        .expect("module header present");
    let mod_section = &prompt[mod_idx..];
    assert!(
        mod_section.contains("```toml"),
        "module section should be TOML-fenced"
    );
}
