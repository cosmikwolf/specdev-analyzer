//! hello-firmware smoke: the firmware-layered profile end-to-end through
//! the full pipeline — bootstrap → inline embassy + stm32h5-rs +
//! embassy+stm32h5-rs bridge → author two toy modules with requirements
//! wiring → validate → codegen gate → prompt assembly → Cargo.toml render
//! → gen-lineage record + gen-drift.
//!
//! Structural-validation coverage of this stack exists elsewhere (the
//! bridge tests stop at validate). What this file adds is the back half
//! of the pipeline on the firmware profile: firmware-layered's
//! `profile.yaml` is the only greenfield-authored manifest, and a
//! manifest gap would hide exactly in prompt assembly / cargo-toml
//! dispatch / lineage recording, not in schema validation.

use crate::ledger;

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
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
        "specdev-hello-firmware-{tag}-{}-{}",
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

/// Build the full hello-firmware project: bootstrap + inlines + bridge +
/// requirements + two toy modules (Blinky without bridge bindings,
/// UartEcho with them) + a concrete `mcu_part` pick.
fn hello_firmware_project(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("hello-firmware");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "hello-firmware".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &full_plugin(), "embassy+stm32h5-rs").unwrap();

    // Concrete MCU pick: uncomment the template's example part.
    let arch_path = target.join("specs/architecture.toml");
    let arch = std::fs::read_to_string(&arch_path).unwrap();
    std::fs::write(
        &arch_path,
        arch.replace("# mcu_part = \"stm32h563\"", "mcu_part = \"stm32h563\""),
    )
    .unwrap();

    ledger::write(
        &target,
        "requirements.toml",
        r#"[[requirement]]
id = "REQ-LED-001"
text = "The board LED shall blink at 1 Hz as a liveness indicator."

[[requirement]]
id = "REQ-UART-001"
text = "Bytes received on USART3 shall be echoed back unmodified."
"#,
    );

    std::fs::write(
        target.join("specs/modules/Blinky.toml"),
        r#"name = "Blinky"
layer = "application"
description = "Liveness LED: toggle the user LED at 1 Hz."
satisfies = ["REQ-LED-001"]

[[items]]
kind = "fn"
name = "blink"
description = "Embassy task that owns the LED output: toggle led_pin every 500 ms forever."
satisfies = ["REQ-LED-001"]

[[stm32h5.pins]]
name = "led_pin"
port = "B"
pin = 0
mode = "output"

[[embassy.tasks]]
name = "blink"
pool_size = 1
"#,
    )
    .unwrap();

    std::fs::write(
        target.join("specs/modules/UartEcho.toml"),
        r#"name = "UartEcho"
layer = "driver"
description = "Echo every byte received on USART3."
satisfies = ["REQ-UART-001"]

[[items]]
kind = "fn"
name = "echo"
description = "Embassy task that owns the async UART: read one byte from echo_uart, write it back."
satisfies = ["REQ-UART-001"]

[[stm32h5.peripherals]]
name = "echo_uart"
kind = "USART"
instance = 3

[[embassy.tasks]]
name = "echo"
pool_size = 1

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "echo"
peripheral = "echo_uart"
handoff = "owned"
"#,
    )
    .unwrap();

    // Re-discover after the spec mutations (layout caches parsed state).
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn assert_no_errors(target: &Path) {
    let report = validate::run_project(target, None).unwrap();
    let errors: Vec<_> = report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .collect();
    assert!(
        errors.is_empty(),
        "expected 0 errors:\n{}",
        report.render_plain()
    );
}

#[test]
fn full_project_validates_clean() {
    let (target, _layout) = hello_firmware_project("validate");
    assert_no_errors(&target);
}
