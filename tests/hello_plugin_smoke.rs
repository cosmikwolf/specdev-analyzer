//! hello-plugin smoke fixture.
//!
//! The regression target for the full audio-plugin stack. Builds a
//! workspace project programmatically:
//!
//!   hello-plugin/
//!   ├── Cargo.toml                # workspace root, emitted by workspace render
//!   ├── spec.toml                 # [workspace], [project] = workspace pkg
//!   ├── specs/architecture.toml   # workspace_package + workspace_dependencies
//!   ├── crates/
//!   │   ├── engine/               # library-api member
//!   │   │   ├── Cargo.toml        # [lib] only; no_std-eligible
//!   │   │   ├── spec.toml
//!   │   │   ├── specs/architecture.toml + modules/SineOsc.toml
//!   │   └── plugin/               # audio-plugin member
//!   │       ├── Cargo.toml        # [lib] cdylib + [[bin]] standalone
//!   │       ├── spec.toml
//!   │       ├── specs/architecture.toml ([plugin]/[audio]/[runtime]) +
//!   │       │   modules/MyPlugin.toml ([nih_plug] + [egui] + bindings)
//!
//! Exit criteria for this hermetic fixture (a cargo build live against
//! the nih-plug git rev belongs in a separate dogfood, not here):
//!   - Workspace bootstraps clean.
//!   - Engine member bootstraps with library-api; validates clean.
//!   - Plugin member bootstraps with audio-plugin; inlines nih-plug +
//!     egui blocks + egui+nih-plug bridge; validates clean.
//!   - Workspace-root Cargo.toml renders [workspace] +
//!     [workspace.package] + [workspace.dependencies].
//!   - Engine Cargo.toml renders as library-api ([lib], no [[bin]]).
//!   - Plugin Cargo.toml renders as audio-plugin
//!     ([lib] cdylib + [[bin]] <plugin>-standalone).
//!   - Plugin module's `prompt` assembly composes profile + nih-plug +
//!     egui + egui+nih-plug + architecture + module spec.

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
        "specdev-hello-plugin-smoke-{tag}-{}-{}",
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

fn errors(report: &Report) -> Vec<Finding> {
    report
        .findings()
        .iter()
        .filter(|f| matches!(f.severity, Severity::Error))
        .cloned()
        .collect()
}

/// Build a hello-plugin workspace at `<tag>/proj/`. Returns the
/// workspace root path. Bootstrap order: workspace → engine member →
/// plugin member → blocks (in plugin) → bridge (in plugin) → modules.
fn build_hello_plugin(tag: &str) -> PathBuf {
    let workspace_dir = tmp_workspace(tag);
    let target = workspace_dir.join("proj");
    std::fs::create_dir_all(&target).unwrap();

    // 1. Bootstrap workspace root.
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "rust-workspace".into(),
            language: "rust".into(),
            project_name: "hello-plugin".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    // Overwrite workspace spec.toml with two members.
    std::fs::write(
        target.join("spec.toml"),
        r#"spec_version = "2.0"

[project]
name = "hello-plugin"
profile = "rust-workspace"
language = "rust"

[paths]
specs = "specs"
source = "."

[workspace]
resolver = "2"

[[workspace.members]]
name = "engine"
path = "crates/engine"

[[workspace.members]]
name = "plugin"
path = "crates/plugin"
"#,
    )
    .unwrap();

    // Workspace-level architecture: workspace_package + (no shared
    // workspace_dependencies in v1; per-member crates own their deps).
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"[workspace_package]
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
"#,
    )
    .unwrap();

    // 2. Bootstrap engine member (library-api).
    let engine_dir = target.join("crates").join("engine");
    std::fs::create_dir_all(&engine_dir).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "engine".into(),
            target_dir: engine_dir.clone(),
        },
    )
    .unwrap();

    // Engine architecture: minimal library-api shape; one optional
    // feature (no_std-style toggle).
    std::fs::write(
        engine_dir.join("specs/architecture.toml"),
        r#"# Engine architecture: minimal library-api shape.

[[features]]
name = "std"
description = "Use std (otherwise no_std)."
default = true
"#,
    )
    .unwrap();

    // Engine module: one DSP node — SineOsc.
    std::fs::write(
        engine_dir.join("specs/modules/SineOsc.toml"),
        r#"name = "SineOsc"
visibility = "pub"
description = "Sine-wave oscillator for the hello-plugin engine."
"#,
    )
    .unwrap();

    // 3. Bootstrap plugin member (audio-plugin).
    let plugin_dir = target.join("crates").join("plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "audio-plugin".into(),
            language: "rust".into(),
            project_name: "plugin".into(),
            target_dir: plugin_dir.clone(),
        },
    )
    .unwrap();

    // Plugin architecture: [plugin] / [audio] / [runtime] populated;
    // declares vst3 + clap + standalone formats.
    std::fs::write(
        plugin_dir.join("specs/architecture.toml"),
        r#"[plugin]
name = "Hello Plugin"
vendor = "ExampleCo"
version = "0.1.0"
category = "instrument"
formats = ["vst3", "clap", "standalone"]

[audio]
default_channel_layout = "stereo"

[runtime]
panic_handler = "abort"
"#,
    )
    .unwrap();

    // 4. Inline nih-plug + egui blocks + bridge in plugin member.
    let plugin_layout = LayoutContext::discover(&plugin_dir, None).unwrap();
    run_inline_block(&plugin_layout, &full_plugin(), "nih-plug").unwrap();
    run_inline_block(&plugin_layout, &full_plugin(), "egui").unwrap();
    let plugin_layout = LayoutContext::discover(&plugin_dir, None).unwrap();
    run_inline_bridge(&plugin_layout, &full_plugin(), "egui+nih-plug").unwrap();

    // 5. Plugin module: MyPlugin shell with [nih_plug] + [egui] +
    // bindings."egui+nih-plug" — one parameter, one widget, one region.
    std::fs::write(
        plugin_dir.join("specs/modules/MyPlugin.toml"),
        r#"name = "MyPlugin"
layer = "plugin_shell"
description = "Hello-plugin shell — exposes a gain knob driving a sine."

[[nih_plug.parameters]]
id = "gain"
name = "Gain"
kind = "float"
min = -30.0
max = 30.0
default = 0.0
unit = "dB"
smoother = "logarithmic"

[[nih_plug.audio_ports]]
name = "main_in"
direction = "input"
channel_count = 2

[[nih_plug.audio_ports]]
name = "main_out"
direction = "output"
channel_count = 2

[[egui.panels]]
id = "main"
kind = "central"

[[egui.regions]]
id = "param_grid"
panel = "main"
purpose = "parameter_controls"
layout = "grid"
columns = 2

[[bindings."egui+nih-plug".parameter_widgets]]
parameter = "gain"
region = "param_grid"
widget_kind = "knob"
range_hint = "logarithmic"
"#,
    )
    .unwrap();

    target
}

// ---------------------------------------------------------------------------
// 1 — Bootstrap layout: workspace + 2 members all scaffold the expected
//     directory structure.
// ---------------------------------------------------------------------------
#[test]
fn hello_plugin_workspace_bootstraps_with_two_members() {
    let target = build_hello_plugin("bootstrap-layout");

    // Workspace root pieces.
    assert!(target.join("spec.toml").is_file());
    assert!(target.join("specs/architecture.toml").is_file());
    assert!(target.join("specs/specdev").is_dir());

    // Engine member.
    assert!(target.join("crates/engine/spec.toml").is_file());
    assert!(target
        .join("crates/engine/specs/modules/SineOsc.toml")
        .is_file());
    assert!(
        target
            .join("crates/engine/specs/specdev/schemas/module.schema.yaml")
            .is_file(),
        "engine inherits library-api module schema"
    );

    // Plugin member.
    assert!(target.join("crates/plugin/spec.toml").is_file());
    assert!(target
        .join("crates/plugin/specs/modules/MyPlugin.toml")
        .is_file());
    assert!(
        target
            .join("crates/plugin/specs/specdev/schemas/inlined/blocks/nih-plug.schema.yaml")
            .is_file(),
        "plugin has nih-plug inlined"
    );
    assert!(
        target
            .join("crates/plugin/specs/specdev/schemas/inlined/blocks/egui.schema.yaml")
            .is_file(),
        "plugin has egui inlined"
    );
    assert!(
        target
            .join("crates/plugin/specs/specdev/schemas/inlined/bridges/egui+nih-plug.schema.yaml")
            .is_file(),
        "plugin has egui+nih-plug bridge inlined"
    );
}

// ---------------------------------------------------------------------------
// 2 — Engine member validates clean (library-api with one DSP node module).
// ---------------------------------------------------------------------------
#[test]
fn engine_member_validates_clean() {
    let target = build_hello_plugin("engine-validate");
    let report = validate::run_project(&target.join("crates/engine"), None).unwrap();
    let errs = errors(&report);
    assert!(
        errs.is_empty(),
        "expected engine member to validate clean; got {errs:#?}"
    );
}

// ---------------------------------------------------------------------------
// 3 — Plugin member validates clean (audio-plugin + nih-plug + egui +
//     bridge bindings all compose).
// ---------------------------------------------------------------------------
#[test]
fn plugin_member_validates_clean() {
    let target = build_hello_plugin("plugin-validate");
    let report = validate::run_project(&target.join("crates/plugin"), None).unwrap();
    let errs = errors(&report);
    assert!(
        errs.is_empty(),
        "expected plugin member to validate clean; got {errs:#?}"
    );
}
