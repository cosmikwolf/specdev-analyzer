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
    cargo_toml::render,
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
    prompt,
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
        r#"spec_version = "1.0"

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
    assert!(target.join("crates/engine/specs/modules/SineOsc.toml").is_file());
    assert!(
        target
            .join("crates/engine/specs/specdev/schemas/module.schema.yaml")
            .is_file(),
        "engine inherits library-api module schema"
    );

    // Plugin member.
    assert!(target.join("crates/plugin/spec.toml").is_file());
    assert!(target.join("crates/plugin/specs/modules/MyPlugin.toml").is_file());
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

// ---------------------------------------------------------------------------
// 4 — Workspace-root Cargo.toml renders [workspace] + members +
//     [workspace.package].
// ---------------------------------------------------------------------------
#[test]
fn workspace_root_cargo_toml_renders() {
    let target = build_hello_plugin("ws-root-render");
    let layout = LayoutContext::discover(&target, None).unwrap();
    let rendered = render(&layout).unwrap();
    let t = &rendered.text;

    assert!(t.starts_with("[workspace]\n"), "{t}");
    assert!(t.contains("resolver = \"2\"\n"), "{t}");
    assert!(
        t.contains("\"crates/engine\"") && t.contains("\"crates/plugin\""),
        "members listed:\n{t}"
    );
    assert!(t.contains("[workspace.package]\n"), "{t}");
    assert!(
        t.contains("version = \"0.1.0\"\n") && t.contains("edition = \"2021\"\n"),
        "[workspace.package] populated:\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 5 — Engine Cargo.toml renders as library-api: [package] + [lib] +
//     no [[bin]] target.
// ---------------------------------------------------------------------------
#[test]
fn engine_cargo_toml_renders_as_library() {
    let target = build_hello_plugin("engine-cargo");
    let engine_layout = LayoutContext::discover(&target.join("crates/engine"), None).unwrap();
    let rendered = render(&engine_layout).unwrap();
    let t = &rendered.text;

    assert!(t.starts_with("[package]\n"), "{t}");
    assert!(t.contains("name = \"engine\"\n"), "{t}");
    assert!(t.contains("[lib]\n"), "{t}");
    assert!(
        !t.contains("[[bin]]"),
        "library-api should not emit [[bin]]:\n{t}"
    );
    assert!(
        !t.contains("crate-type = [\"cdylib\"]"),
        "library-api should not emit cdylib:\n{t}"
    );
    // [features] section: synthesized `default = ["std"]` line + std = [].
    assert!(t.contains("[features]\n"), "{t}");
    assert!(t.contains("default = [\"std\"]\n"), "{t}");
}

// ---------------------------------------------------------------------------
// 6 — Plugin Cargo.toml renders as audio-plugin: [package] + [lib]
//     cdylib + [[bin]] <project>-standalone.
// ---------------------------------------------------------------------------
#[test]
fn plugin_cargo_toml_renders_cdylib_plus_standalone_bin() {
    let target = build_hello_plugin("plugin-cargo");
    let plugin_layout = LayoutContext::discover(&target.join("crates/plugin"), None).unwrap();
    let rendered = render(&plugin_layout).unwrap();
    let t = &rendered.text;

    assert!(t.starts_with("[package]\n"), "{t}");
    assert!(t.contains("name = \"plugin\"\n"), "{t}");
    assert!(
        t.contains("[lib]\ncrate-type = [\"cdylib\"]\n"),
        "expected cdylib lib:\n{t}"
    );
    assert!(
        t.contains("[[bin]]\nname = \"plugin-standalone\"\npath = \"src/bin/standalone.rs\"\n"),
        "expected standalone bin target:\n{t}"
    );
}

// ---------------------------------------------------------------------------
// 7 — Plugin module's prompt assembly composes profile + 2 blocks +
//     bridge prose + architecture + module spec into one deterministic
//     prompt string.
// ---------------------------------------------------------------------------
#[test]
fn plugin_module_prompt_composes_full_stack() {
    let target = build_hello_plugin("prompt-compose");
    let plugin_layout = LayoutContext::discover(&target.join("crates/plugin"), None).unwrap();

    let assembled = prompt::assemble_module_prompt(&plugin_layout, "MyPlugin").unwrap();
    let p = &assembled;

    // Profile prose is in there (audio-plugin module.conventions.md).
    assert!(
        p.contains("audio-plugin: Rust module conventions"),
        "expected audio-plugin profile prose"
    );
    // nih-plug block prose is in there.
    assert!(
        p.contains("nih-plug — Rust codegen conventions"),
        "expected nih-plug block prose"
    );
    // egui block prose is in there.
    assert!(
        p.contains("egui — Rust codegen conventions"),
        "expected egui block prose"
    );
    // Bridge prose is in there.
    assert!(
        p.contains("egui+nih-plug — Rust codegen conventions"),
        "expected egui+nih-plug bridge prose"
    );
    // Architecture.toml content fenced into the prompt.
    assert!(
        p.contains("[plugin]") && p.contains("Hello Plugin") && p.contains("[audio]"),
        "expected architecture.toml fenced into prompt"
    );
    // Module spec content fenced into the prompt.
    assert!(
        p.contains("name = \"MyPlugin\"") && p.contains("[[nih_plug.parameters]]"),
        "expected MyPlugin module spec fenced into prompt"
    );
    // Bindings table appears.
    assert!(
        p.contains("bindings.\"egui+nih-plug\""),
        "expected bindings table fenced into prompt"
    );
}

// ---------------------------------------------------------------------------
// 8 — Architectural validation marker: a single hello-plugin workspace
//     fixture exercises the full audio-plugin stack from bootstrap through
//     prompt-assembly without errors.
// ---------------------------------------------------------------------------
#[test]
fn full_audio_plugin_stack_composes_end_to_end() {
    let target = build_hello_plugin("full-stack");

    // Workspace + engine + plugin all discoverable.
    let _ws = LayoutContext::discover(&target, None).unwrap();
    let engine_layout = LayoutContext::discover(&target.join("crates/engine"), None).unwrap();
    let plugin_layout = LayoutContext::discover(&target.join("crates/plugin"), None).unwrap();

    // Both members validate.
    let engine_report = validate::run_project(&target.join("crates/engine"), None).unwrap();
    assert!(
        errors(&engine_report).is_empty(),
        "engine validate clean; got {:#?}",
        errors(&engine_report)
    );
    let plugin_report = validate::run_project(&target.join("crates/plugin"), None).unwrap();
    assert!(
        errors(&plugin_report).is_empty(),
        "plugin validate clean; got {:#?}",
        errors(&plugin_report)
    );

    // Both members render Cargo.toml.
    let engine_cargo = render(&engine_layout).unwrap();
    assert!(engine_cargo.text.contains("[lib]"), "engine has [lib]");
    let plugin_cargo = render(&plugin_layout).unwrap();
    assert!(
        plugin_cargo.text.contains("[lib]\ncrate-type = [\"cdylib\"]"),
        "plugin has cdylib"
    );
    assert!(
        plugin_cargo.text.contains("[[bin]]\nname = \"plugin-standalone\""),
        "plugin has standalone bin"
    );

    // Plugin prompt assembles.
    let prompt_text = prompt::assemble_module_prompt(&plugin_layout, "MyPlugin").unwrap();
    assert!(prompt_text.len() > 5_000, "prompt is non-trivial size");
    assert!(prompt_text.contains("egui+nih-plug"), "bridge composed in");
}
