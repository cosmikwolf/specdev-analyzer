//! Embedded schemas + manifest loaders.
//!
//! Round-trip tests for the 10 manifests in the analyzer/content contract:
//!   - 4 profile.yaml — library-api / firmware-layered / audio-plugin / rust-workspace
//!   - 4 block deps.yaml — nih-plug / egui / embassy / stm32h5-rs
//!   - 2 bridge contract.yaml — egui+nih-plug / embassy+stm32h5-rs
//!
//! Plus rejection tests for closed-enum violations (proves the schema
//! validators are doing their job).

use std::path::PathBuf;

use specdev_analyzer::manifest::{
    BindingLocality, BlockDeps, BootstrapStep, BridgeContract, CrateSource, Profile, ShapeKind,
};

fn tmp_path(tag: &str, ext: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-manifest-loaders-{tag}-{}-{}.{ext}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_file(&p);
    p
}

fn write(tag: &str, body: &str) -> PathBuf {
    let p = tmp_path(tag, "yaml");
    std::fs::write(&p, body).unwrap();
    p
}

// ===========================================================================
// 1 — profile.yaml round-trip × 4
// ===========================================================================

#[test]
fn profile_library_api_roundtrips() {
    let body = r#"
name: library-api
language: rust
cargo_toml:
  shape: library
  default_edition: "2021"
  lib: {}
  features:
    synthesize_default: true
    drain_from: ["features"]
  architecture_deps:
    typed_slots: []
    open_arrays: ["external_deps"]
bootstrap:
  steps: []
context:
  module_fields_for_prompt:
    [items, satisfies, depends_on, context_files, external_deps]
  context_subcommand_extras:
    - field: visibility
      from_attr: visibility
surface:
  has_modules: true
  has_requirements: true
  has_architecture: true
"#;
    let p = write("profile-library-api", body);
    let m = Profile::load(&p).expect("library-api profile loads");
    assert_eq!(m.name, "library-api");
    assert_eq!(m.language, "rust");
    let cargo = m.cargo_toml.expect("cargo_toml present");
    assert_eq!(cargo.shape, ShapeKind::Library);
    assert_eq!(cargo.default_edition.as_deref(), Some("2021"));
    let surf = m.surface.expect("surface present");
    assert!(surf.has_modules && surf.has_requirements && surf.has_architecture);
}

#[test]
fn profile_firmware_layered_roundtrips() {
    let body = r#"
name: firmware-layered
language: rust
cargo_toml:
  shape: binary
  default_edition: "2021"
  bin:
    name_template: "{project_name}"
    path_template: "src/main.rs"
  features:
    synthesize_default: false
    drain_from: []
  architecture_deps:
    typed_slots:
      - table: "platform"
        keys: ["mcu_core", "mcu_runtime"]
      - table: "runtime"
        keys: ["panic_handler", "allocator", "critical_section_impl"]
      - table: "logger"
        keys: ["provider", "backend"]
    open_arrays: ["external_deps"]
bootstrap:
  steps: []
context:
  module_fields_for_prompt:
    [items, satisfies, depends_on, context_files, external_deps, layer]
  context_subcommand_extras:
    - field: layer
      from_attr: layer
    - field: hardware_resources
      kind: collect_local_entities_by_kind
      kinds: ["Peripheral", "Pin"]
surface:
  has_modules: true
  has_requirements: true
  has_architecture: true
"#;
    let p = write("profile-firmware-layered", body);
    let m = Profile::load(&p).expect("firmware-layered profile loads");
    let cargo = m.cargo_toml.unwrap();
    assert_eq!(cargo.shape, ShapeKind::Binary);
    let bin = cargo.bin.expect("bin section present");
    assert_eq!(bin.name_template, "{project_name}");
    assert_eq!(bin.path_template, "src/main.rs");
    let arch_deps = cargo.architecture_deps.unwrap();
    assert_eq!(arch_deps.typed_slots.len(), 3);
    assert_eq!(arch_deps.typed_slots[0].table, "platform");
    let ctx = m.context.unwrap();
    assert_eq!(ctx.context_subcommand_extras.len(), 2);
    let hw_extra = &ctx.context_subcommand_extras[1];
    assert_eq!(hw_extra.field, "hardware_resources");
    assert_eq!(hw_extra.kind.as_deref(), Some("collect_local_entities_by_kind"));
    assert_eq!(hw_extra.kinds, vec!["Peripheral", "Pin"]);
}

#[test]
fn profile_audio_plugin_roundtrips() {
    let body = r#"
name: audio-plugin
language: rust
cargo_toml:
  shape: plugin
  default_edition: "2021"
  lib:
    crate_type: ["cdylib"]
    cdylib_only: true
  bin:
    name_template: "{project_name}-standalone"
    path_template: "src/bin/standalone.rs"
    gate:
      kind: architecture_array_contains
      table: "plugin"
      field: "formats"
      value: "standalone"
    paired_stub:
      template_file: templates/standalone-stub.rs.template
  features:
    synthesize_default: true
    drain_from: ["features"]
  architecture_deps:
    typed_slots: []
    open_arrays: ["external_deps"]
bootstrap:
  steps: []
context:
  module_fields_for_prompt:
    [items, satisfies, depends_on, context_files, external_deps, layer,
     nih_plug, egui]
  context_subcommand_extras:
    - field: layer
      from_attr: layer
surface:
  has_modules: true
  has_requirements: true
  has_architecture: true
"#;
    let p = write("profile-audio-plugin", body);
    let m = Profile::load(&p).expect("audio-plugin profile loads");
    let cargo = m.cargo_toml.unwrap();
    assert_eq!(cargo.shape, ShapeKind::Plugin);
    let lib = cargo.lib.expect("lib section present");
    assert!(lib.cdylib_only);
    let bin = cargo.bin.expect("bin section present");
    let stub = bin.paired_stub.expect("paired_stub present");
    assert_eq!(stub.template_file, "templates/standalone-stub.rs.template");
}

#[test]
fn profile_rust_workspace_roundtrips() {
    let body = r#"
name: rust-workspace
language: rust
cargo_toml:
  shape: workspace
  default_edition: "2021"
  features:
    synthesize_default: false
    drain_from: []
  architecture_deps:
    typed_slots: []
    open_arrays: []
bootstrap:
  steps: [emit_workspace_root, emit_member_stubs, walk_members]
context:
  module_fields_for_prompt: []
  context_subcommand_extras: []
surface:
  has_modules: false
  has_requirements: false
  has_architecture: true
"#;
    let p = write("profile-rust-workspace", body);
    let m = Profile::load(&p).expect("rust-workspace profile loads");
    assert_eq!(m.cargo_toml.unwrap().shape, ShapeKind::Workspace);
    let boot = m.bootstrap.unwrap();
    assert_eq!(boot.steps.len(), 3);
    assert_eq!(boot.steps[0], BootstrapStep::EmitWorkspaceRoot);
    assert_eq!(boot.steps[1], BootstrapStep::EmitMemberStubs);
    assert_eq!(boot.steps[2], BootstrapStep::WalkMembers);
    let surf = m.surface.unwrap();
    assert!(!surf.has_modules);
    assert!(!surf.has_requirements);
    assert!(surf.has_architecture);
}

// ===========================================================================
// 2 — block deps.yaml round-trip × 4
// ===========================================================================

#[test]
fn block_deps_nih_plug_roundtrips() {
    let body = r#"
crates:
  - name: nih_plug
    version: "0.0"
    features: []
    source:
      git: "https://github.com/robbert-vdh/nih-plug"
      rev: "abc123"
recommended_external_deps:
  - crate: nih_plug
    purpose: "plugin trait + parameter system"
"#;
    let p = write("deps-nih-plug", body);
    let m = BlockDeps::load(&p).expect("nih-plug deps load");
    assert_eq!(m.crates.len(), 1);
    let c = &m.crates[0];
    assert_eq!(c.name, "nih_plug");
    assert!(matches!(&c.source, Some(CrateSource::Git { git, rev: Some(_), .. }) if git.contains("nih-plug")));
    assert_eq!(m.recommended_external_deps.len(), 1);
    assert_eq!(m.recommended_external_deps[0].crate_name, "nih_plug");
}

#[test]
fn block_deps_egui_roundtrips() {
    let body = r#"
crates:
  - name: nih_plug_egui
    version: "0.0"
    features: []
    source:
      git: "https://github.com/robbert-vdh/nih-plug"
      rev: "abc123"
  - name: egui
    version: "0.27"
    source: crates-io
recommended_external_deps:
  - crate: egui_extras
    purpose: "Knob widget when widget_kind is knob"
"#;
    let p = write("deps-egui", body);
    let m = BlockDeps::load(&p).expect("egui deps load");
    assert_eq!(m.crates.len(), 2);
    // Second crate is crates-io tagged variant.
    use specdev_analyzer::manifest::CrateSourceTag;
    assert!(matches!(
        &m.crates[1].source,
        Some(CrateSource::Tagged(CrateSourceTag::CratesIo))
    ));
}

#[test]
fn block_deps_embassy_roundtrips() {
    let body = r#"
crates:
  - name: embassy-executor
    version: "0.6"
    features: ["arch-cortex-m", "executor-thread"]
    source: crates-io
  - name: embassy-time
    version: "0.3"
    source: crates-io
recommended_external_deps: []
"#;
    let p = write("deps-embassy", body);
    let m = BlockDeps::load(&p).expect("embassy deps load");
    assert_eq!(m.crates.len(), 2);
    assert_eq!(m.crates[0].features, vec!["arch-cortex-m", "executor-thread"]);
    assert!(m.recommended_external_deps.is_empty());
}

#[test]
fn block_deps_stm32h5_rs_roundtrips() {
    let body = r#"
crates:
  - name: embassy-stm32
    version: "0.1"
    features: ["unstable-pac"]
    source: crates-io
recommended_external_deps:
  - crate: cortex-m
    purpose: "ARM Cortex-M startup"
  - crate: cortex-m-rt
    purpose: "linker + reset handler"
"#;
    let p = write("deps-stm32h5", body);
    let m = BlockDeps::load(&p).expect("stm32h5-rs deps load");
    assert_eq!(m.recommended_external_deps.len(), 2);
}

#[test]
fn block_deps_load_returns_default_when_file_absent() {
    let p = tmp_path("deps-absent", "yaml");
    let m = BlockDeps::load(&p).expect("absent deps return default");
    assert!(m.crates.is_empty());
    assert!(m.recommended_external_deps.is_empty());
}

// ===========================================================================
// 3 — bridge contract.yaml round-trip × 2
// ===========================================================================

#[test]
fn bridge_contract_egui_nih_plug_roundtrips() {
    let body = r#"
binding_locality: plugin_shell_unique
diagnostic_template: |
  the {bridge_name} bridge requires co-residency on the plugin_shell module:
  parameter `{parameter}` is declared on `{decl_module}` but its widget binding
  is declared on `{binding_module}`.
"#;
    let p = write("contract-egui-nih-plug", body);
    let m = BridgeContract::load(&p).expect("egui+nih-plug contract loads");
    assert_eq!(m.binding_locality, BindingLocality::PluginShellUnique);
    assert!(m.diagnostic_template.contains("{bridge_name}"));
    assert!(m.diagnostic_template.contains("{parameter}"));
}

#[test]
fn bridge_contract_embassy_stm32_roundtrips() {
    let body = r#"
binding_locality: endpoint_module
diagnostic_template: |
  the {bridge_name} bridge requires intra-module wiring.
"#;
    let p = write("contract-embassy-stm32", body);
    let m = BridgeContract::load(&p).expect("embassy+stm32h5-rs contract loads");
    assert_eq!(m.binding_locality, BindingLocality::EndpointModule);
}

// ===========================================================================
// 4 — closed-enum rejection
// ===========================================================================

#[test]
fn profile_rejects_unknown_shape() {
    let body = r#"
name: bogus
language: rust
cargo_toml:
  shape: chimera
"#;
    let p = write("profile-bad-shape", body);
    let err = Profile::load(&p).expect_err("unknown shape should fail");
    let msg = format!("{err:#}");
    assert!(msg.contains("MANIFEST-001"), "{msg}");
    assert!(msg.contains("chimera") || msg.contains("shape"), "{msg}");
}

#[test]
fn profile_rejects_unknown_top_level_field() {
    let body = r#"
name: lib
language: rust
unknown_field: 42
"#;
    let p = write("profile-bad-top", body);
    let err = Profile::load(&p).expect_err("unknown top-level field should fail");
    assert!(format!("{err:#}").contains("MANIFEST-001"));
}

#[test]
fn block_deps_rejects_empty_purpose() {
    let body = r#"
crates: []
recommended_external_deps:
  - crate: foo
    purpose: ""
"#;
    let p = write("deps-empty-purpose", body);
    let err = BlockDeps::load(&p).expect_err("empty purpose should fail");
    assert!(format!("{err:#}").contains("MANIFEST-001"));
}

#[test]
fn bridge_contract_rejects_unknown_binding_locality() {
    let body = r#"
binding_locality: ambient_disco
diagnostic_template: "irrelevant"
"#;
    let p = write("contract-bad-locality", body);
    let err = BridgeContract::load(&p).expect_err("unknown locality should fail");
    assert!(format!("{err:#}").contains("MANIFEST-001"));
}

#[test]
fn bridge_contract_rejects_co_resident_dropped_value() {
    // co_resident was an early draft value dropped before commit; the
    // closed enum must reject it.
    let body = r#"
binding_locality: co_resident
diagnostic_template: "irrelevant"
"#;
    let p = write("contract-coresident-dropped", body);
    let err = BridgeContract::load(&p).expect_err("co_resident must be rejected");
    assert!(format!("{err:#}").contains("MANIFEST-001"));
}
