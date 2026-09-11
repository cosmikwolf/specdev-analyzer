//! Deterministic prompt assembly.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
    prompt::{
        assemble_module_prompt, assemble_preamble, assemble_tail, detect_relevance,
        list_module_names,
    },
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
    assert!(
        rel.blocks.is_empty(),
        "no blocks expected; got {:?}",
        rel.blocks
    );
    assert!(
        rel.bridges.is_empty(),
        "no bridges expected; got {:?}",
        rel.bridges
    );

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
    .map(|h| {
        prompt
            .find(h)
            .unwrap_or_else(|| panic!("missing header: {h}"))
    })
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

// 9 — whole prompt == preamble(&[m]) + tail(m), byte-for-byte.
#[test]
fn whole_prompt_equals_preamble_plus_tail() {
    let (target, layout) = fresh_firmware_project("split-eq");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target);

    let whole = assemble_module_prompt(&layout, "Uart").unwrap();
    let preamble = assemble_preamble(&layout, &["Uart".to_string()]).unwrap();
    let tail = assemble_tail(&layout, "Uart").unwrap();
    assert_eq!(
        whole,
        format!("{preamble}{tail}"),
        "whole prompt must equal preamble + tail byte-for-byte"
    );
}

// 10 — preamble carries Tier 1 + Tier 2, never Tier 3.
#[test]
fn preamble_excludes_tier3() {
    let (target, layout) = fresh_firmware_project("preamble-t3");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target);

    let preamble = assemble_preamble(&layout, &["Uart".to_string()]).unwrap();
    assert!(preamble.contains("## Profile module conventions"));
    assert!(preamble.contains("## Architecture (`specs/architecture.toml`)"));
    assert!(preamble.contains("## Block: `embassy`"));
    assert!(
        !preamble.contains("## Module spec"),
        "preamble must not carry the module spec (Tier 3)"
    );
    assert!(
        !preamble.contains("# Codegen prompt for module"),
        "preamble must not carry the per-module header (Tier 3)"
    );
}

// 11 — tail carries Tier 3, never Tier 1 / Tier 2.
#[test]
fn tail_excludes_tier1_and_tier2() {
    let (target, layout) = fresh_firmware_project("tail-t12");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target);

    let tail = assemble_tail(&layout, "Uart").unwrap();
    assert!(tail.contains("## Module spec (`specs/modules/Uart.toml`)"));
    assert!(tail.contains("# Codegen prompt for module `Uart`"));
    assert!(
        !tail.contains("## Profile module conventions"),
        "tail must not carry conventions (Tier 1)"
    );
    assert!(
        !tail.contains("## Architecture"),
        "tail must not carry architecture (Tier 1)"
    );
    assert!(
        !tail.contains("## Block:"),
        "tail must not carry block prose (Tier 2)"
    );
}

// 12 — preamble unions block/bridge relevance across a module set; a
// single-module preamble is scoped to that module's relevance.
#[test]
fn preamble_unions_relevance_across_modules() {
    let (target, layout) = fresh_firmware_project("union");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target); // embassy + stm32h5 + bridge
    write_embassy_only_module(&target); // embassy only

    // Single-module scope: Worker sees only embassy, no stm32h5, no bridge.
    let worker_only = assemble_preamble(&layout, &["Worker".to_string()]).unwrap();
    assert!(worker_only.contains("## Block: `embassy`"));
    assert!(!worker_only.contains("## Block: `stm32h5-rs`"));
    assert!(!worker_only.contains("## Bridge:"));

    // Union over both: every block + the bridge appears exactly once.
    let union = assemble_preamble(&layout, &["Uart".to_string(), "Worker".to_string()]).unwrap();
    assert!(union.contains("## Block: `embassy`"));
    assert!(union.contains("## Block: `stm32h5-rs`"));
    assert!(union.contains("## Bridge: `embassy+stm32h5-rs`"));
    assert_eq!(
        union.matches("## Block: `embassy`").count(),
        1,
        "shared block must be deduped in the union"
    );
}

// 13 — empty module set yields conventions + architecture only (27-D
// cold-start: a project with profile prose but no modules yet).
#[test]
fn empty_preamble_is_conventions_and_architecture_only() {
    let (_target, layout) = fresh_firmware_project("empty-preamble");
    let preamble = assemble_preamble(&layout, &[]).unwrap();
    assert!(preamble.contains("## Profile module conventions"));
    assert!(preamble.contains("## Architecture (`specs/architecture.toml`)"));
    assert!(!preamble.contains("## Block:"));
    assert!(!preamble.contains("## Bridge:"));
}

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

// 15 — `gen --preamble` (no --module) unions over all modules, gate-free,
// exit 0; `gen --module <N> --tail` emits Tier 3 only, exit 0; the two
// concatenated reproduce the single-module whole prompt when the project
// has exactly one module.
#[test]
fn cli_preamble_and_tail_flags() {
    let (target, layout) = fresh_firmware_project("cli-split");
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();
    write_uart_module(&target);
    let bin = env!("CARGO_BIN_EXE_specdev-analyzer");

    let preamble = std::process::Command::new(bin)
        .args(["gen", "--preamble", "--project"])
        .arg(&target)
        .output()
        .expect("gen --preamble");
    assert!(
        preamble.status.success(),
        "gen --preamble failed: {}",
        String::from_utf8_lossy(&preamble.stderr)
    );
    let preamble_s = String::from_utf8(preamble.stdout).unwrap();
    assert!(preamble_s.contains("## Profile module conventions"));
    assert!(!preamble_s.contains("## Module spec"));

    let tail = std::process::Command::new(bin)
        .args(["gen", "--module", "Uart", "--tail", "--project"])
        .arg(&target)
        .output()
        .expect("gen --module Uart --tail");
    assert!(
        tail.status.success(),
        "gen --tail failed: {}",
        String::from_utf8_lossy(&tail.stderr)
    );
    let tail_s = String::from_utf8(tail.stdout).unwrap();
    assert!(tail_s.contains("## Module spec (`specs/modules/Uart.toml`)"));
    assert!(!tail_s.contains("## Profile module conventions"));

    // Single-module project: preamble(all) + tail == whole prompt.
    let whole = assemble_module_prompt(&layout, "Uart").unwrap();
    assert_eq!(format!("{preamble_s}{tail_s}"), whole);
}

// 16 — `--preamble` and `--tail` are mutually exclusive (clap-level).
#[test]
fn cli_preamble_and_tail_conflict() {
    let (target, _layout) = fresh_firmware_project("cli-conflict");
    write_uart_module(&target);
    let bin = env!("CARGO_BIN_EXE_specdev-analyzer");
    let out = std::process::Command::new(bin)
        .args([
            "gen",
            "--module",
            "Uart",
            "--preamble",
            "--tail",
            "--project",
        ])
        .arg(&target)
        .output()
        .expect("gen --preamble --tail");
    assert!(
        !out.status.success(),
        "--preamble + --tail must be rejected"
    );
}

// 17 — the codegen gate still fires on `gen --module <N> --tail` when an
// open question is reachable from the module.
#[test]
fn cli_tail_enforces_codegen_gate() {
    let (target, _layout) = fresh_firmware_project("cli-gate");
    // Author a requirement + an open question that affects it, and a
    // module that satisfies the requirement → the gate must refuse.
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"[[requirement]]
id = "REQ-1"
text = "the thing"
layer = "driver"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/open_questions.toml"),
        r#"[[open_question]]
id = "OQ-1"
title = "unresolved"
text = "is the thing X or Y?"
status = "open"
affects = ["REQ-1"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Gated.toml"),
        r#"name = "Gated"
layer = "driver"
satisfies = ["REQ-1"]
"#,
    )
    .unwrap();
    let bin = env!("CARGO_BIN_EXE_specdev-analyzer");
    let out = std::process::Command::new(bin)
        .args(["gen", "--module", "Gated", "--tail", "--project"])
        .arg(&target)
        .output()
        .expect("gen --module Gated --tail");
    assert!(
        !out.status.success(),
        "tail must refuse when a reachable OQ is open"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("OQ-1") && stderr.contains("Refusing to generate"),
        "refusal must name the open question; got: {stderr}"
    );
}
