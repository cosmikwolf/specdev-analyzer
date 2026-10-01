//! hello-rtic smoke: the firmware-layered profile through the pipeline
//! with the CO-INLINED, UN-BRIDGED rtic + mk20dx pair — bootstrap →
//! inline both blocks → author two toy modules exercising the chip
//! block's v2 surface (clock, pins, timers, interrupts) against rtic's
//! task/resource model → validate → codegen gate → prompt assembly →
//! Cargo.toml render.
//!
//! No bridge is inlined because none exists: the pairing's earn-its-keep
//! gate closed it (its would-be binding was not relation-shaped), so
//! this stack is exactly the shape the composition gates exist for —
//! two units nothing else ever resolves together. The lineage tripwire
//! is not re-tested here; it is stack-independent and covered on the
//! embassy firmware stack.

#[path = "common/ledger.rs"]
mod ledger;

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::run_inline_block,
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
        "specdev-hello-rtic-{tag}-{}-{}",
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

/// Build the full hello-rtic project: bootstrap + both blocks + a
/// requirement + two toy modules. Ticker owns the clock preset, a PIT
/// channel with the matching vector, a hardware task, and a shared
/// counter drained by a software task; Blinky owns a pin and a software
/// task with a local resource.
fn hello_rtic_project(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("hello-rtic");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "hello-rtic".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    run_inline_block(&layout, &full_plugin(), "rtic").unwrap();
    run_inline_block(&layout, &full_plugin(), "mk20dx").unwrap();

    ledger::write(&target, "requirements.toml", r#"[[requirement]]
id = "REQ-TICK-001"
text = "A periodic tick shall increment a counter that a lower-priority task drains."

[[requirement]]
id = "REQ-LED-001"
text = "The board LED shall toggle as a liveness indicator."
"#);

    std::fs::write(
        target.join("specs/modules/Ticker.toml"),
        r#"name = "Ticker"
layer = "driver"
description = "1 kHz PIT tick feeding a shared counter across two priorities."
satisfies = ["REQ-TICK-001"]

[[items]]
kind = "fn"
name = "on_tick"
description = "RTIC hardware task in PIT0: clear the timer flag, bump the shared counter."
satisfies = ["REQ-TICK-001"]
signature_hint = "fn on_tick(cx: on_tick::Context)"

[[items]]
kind = "fn"
name = "drain"
description = "RTIC software task: read and reset the shared counter."
satisfies = ["REQ-TICK-001"]
signature_hint = "async fn drain(cx: drain::Context)"

[mk20dx.clock]
speed = "mhz72"

[[mk20dx.timers]]
name = "tick"
channel = 0
period_us = 1000

[[mk20dx.interrupts]]
name = "tick_irq"
vector = "PIT0"

[[rtic.tasks]]
name = "on_tick"
kind = "hardware"
binds = "PIT0"
priority = 3
uses = ["count"]

[[rtic.tasks]]
name = "drain"
kind = "software"
priority = 1
uses = ["count"]

[[rtic.resources]]
name = "count"
sharing = "shared"
type = "u32"
"#,
    )
    .unwrap();

    std::fs::write(
        target.join("specs/modules/Blinky.toml"),
        r#"name = "Blinky"
layer = "application"
description = "Liveness LED toggled by a software task."
satisfies = ["REQ-LED-001"]

[[items]]
kind = "fn"
name = "blink"
description = "RTIC software task: toggle the LED pin, tracking state in a local."
satisfies = ["REQ-LED-001"]
signature_hint = "async fn blink(cx: blink::Context)"

[[mk20dx.pins]]
name = "status_led"
port = "C"
pin = 5
mode = "push_pull_output"

[[rtic.tasks]]
name = "blink"
kind = "software"
priority = 1
uses = ["led_on"]

[[rtic.resources]]
name = "led_on"
sharing = "local"
type = "bool"
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
    let (target, _layout) = hello_rtic_project("validate");
    assert_no_errors(&target);
}
