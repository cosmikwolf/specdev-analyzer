//! Phase 11.5 integration tests: `specdev-analyzer context --module <Name>`.
//!
//! Covers the cone projection + YAML emission per
//! `specdev-next/19-analyzer-context-subcommand.md`. The phase6 plugin
//! fixture (firmware-layered + library-api + embassy + stm32h5-rs +
//! embassy+stm32h5-rs bridge) is the substrate.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    context::{self, ContextReport},
    inline::{run_inline_block, run_inline_bridge},
    layout::LayoutContext,
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
        "specdev-phase11_5-{tag}-{}-{}",
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

fn fresh_library_project(tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn run(layout: &LayoutContext, module: &str) -> ContextReport {
    context::run_context(layout, module).expect("context cone")
}

// ---------------------------------------------------------------------------
// 1 — Module not found returns a usable cold-start response: schema + READMEs
//     present, related_modules empty, status = "module-not-found".
// ---------------------------------------------------------------------------
#[test]
fn module_not_found_returns_cold_start_response() {
    let (_t, layout) = fresh_firmware_project("not-found");
    let r = run(&layout, "DoesNotExist");
    assert_eq!(r.status, "module-not-found");
    assert!(r.related_modules.upstream.is_empty());
    assert!(r.related_modules.downstream.is_empty());
    assert!(r.related_files.is_empty());
    assert!(r.files.target_module.is_none());
    // Schema + module conventions still listed (they don't depend on M).
    assert!(r
        .files
        .schema
        .iter()
        .any(|p| p.ends_with("module.schema.yaml")));
    assert!(r
        .files
        .module_conventions
        .contains("firmware-layered/codegen/rust"));
}

// ---------------------------------------------------------------------------
// 2 — Existing module with no relations: status ok, all buckets empty,
//     target_module set.
// ---------------------------------------------------------------------------
#[test]
fn existing_module_no_relations_clean() {
    let (target, layout) = fresh_firmware_project("plain");
    std::fs::write(
        target.join("specs/modules/Plain.toml"),
        "name = \"Plain\"\nlayer = \"application\"\n",
    )
    .unwrap();
    let r = run(&layout, "Plain");
    assert_eq!(r.status, "ok");
    assert!(r.related_modules.upstream.is_empty());
    assert!(r.related_modules.downstream.is_empty());
    assert!(r.related_modules.binding_endpoints.is_empty());
    assert!(r.related_modules.port_endpoints.is_empty());
    assert_eq!(
        r.files.target_module.as_deref(),
        Some("specs/modules/Plain.toml")
    );
    // profile_specific.layer is set for firmware-layered.
    assert!(r.profile_specific.contains_key("layer"));
}

// ---------------------------------------------------------------------------
// 3 — depends_on populates upstream / downstream and excludes M itself.
// ---------------------------------------------------------------------------
#[test]
fn depends_on_populates_upstream_and_downstream() {
    let (target, layout) = fresh_firmware_project("deps");
    std::fs::write(
        target.join("specs/modules/Lower.toml"),
        "name = \"Lower\"\nlayer = \"driver\"\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Mid.toml"),
        "name = \"Mid\"\nlayer = \"middleware\"\ndepends_on = [\"Lower\"]\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Top.toml"),
        "name = \"Top\"\nlayer = \"application\"\ndepends_on = [\"Mid\"]\n",
    )
    .unwrap();

    let mid = run(&layout, "Mid");
    assert_eq!(mid.related_modules.upstream, vec!["Lower".to_string()]);
    assert_eq!(mid.related_modules.downstream, vec!["Top".to_string()]);
    // Lower and Top are the only siblings of Mid.
    let related: std::collections::BTreeSet<&String> = mid.related_files.iter().collect();
    assert!(related.contains(&"specs/modules/Lower.toml".to_string()));
    assert!(related.contains(&"specs/modules/Top.toml".to_string()));
    // M itself is never in related_files.
    assert!(!related.contains(&"specs/modules/Mid.toml".to_string()));
}

// ---------------------------------------------------------------------------
// 4 — Phase 7 v1 bridges are intra-module: a single module declaring
//     both endpoints + the binding produces zero binding_endpoints
//     (no false positives), but the cone still includes the bridge
//     schema in `files.schema`. Cross-module bindings are bounded-
//     deferred per Phase 7 status; if v1.x relaxes the restriction,
//     extend this test for the cross-module path.
// ---------------------------------------------------------------------------
#[test]
fn intra_module_bridge_does_not_falsely_populate_binding_endpoints() {
    let (target, layout) = fresh_firmware_project("bridge");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &phase6_plugin(), "embassy+stm32h5-rs").unwrap();

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
    let r = run(&layout, "Uart");
    assert!(
        r.related_modules.binding_endpoints.is_empty(),
        "intra-module bridge should not surface as a binding_endpoint; got {:?}",
        r.related_modules.binding_endpoints
    );
    // Bridge schema still surfaces in files.schema so a skill editing
    // Uart's bindings table can validate against it.
    assert!(r
        .files
        .schema
        .iter()
        .any(|p| p.contains("bridges/embassy+stm32h5-rs.schema.yaml")));
}

// ---------------------------------------------------------------------------
// 5 — Architecture connections populate port_endpoints.
// ---------------------------------------------------------------------------
#[test]
fn architecture_connections_populate_port_endpoints() {
    let (target, layout) = fresh_firmware_project("ports");
    std::fs::write(
        target.join("specs/modules/Producer.toml"),
        r#"name = "Producer"
layer = "application"

[[ports]]
name = "out"
direction = "out"
port_type = "u8"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Consumer.toml"),
        r#"name = "Consumer"
layer = "application"

[[ports]]
name = "in"
direction = "in"
port_type = "u8"
"#,
    )
    .unwrap();
    // Append a connection to the bootstrap-provided architecture.toml.
    let arch_path = target.join("specs/architecture.toml");
    let mut arch = std::fs::read_to_string(&arch_path).unwrap();
    arch.push_str(
        r#"
[[connections]]
from = "Producer.out"
to = "Consumer.in"
flow_type = "u8"
"#,
    );
    std::fs::write(&arch_path, arch).unwrap();

    let prod = run(&layout, "Producer");
    assert_eq!(
        prod.related_modules.port_endpoints,
        vec!["Consumer".to_string()]
    );
    let cons = run(&layout, "Consumer");
    assert_eq!(
        cons.related_modules.port_endpoints,
        vec!["Producer".to_string()]
    );
}

// ---------------------------------------------------------------------------
// 6 — Hardware overlap populates hardware_owners on firmware-layered.
// ---------------------------------------------------------------------------
#[test]
fn hardware_overlap_populates_hardware_owners() {
    let (target, layout) = fresh_firmware_project("hw-overlap");
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();

    // Two modules declare Peripheral entries with the same `name`.
    // The overlap signal is "another module declares the same
    // peripheral name" — surfaces as hardware_owners.
    std::fs::write(
        target.join("specs/modules/A.toml"),
        r#"name = "A"
layer = "driver"

[[stm32h5.peripherals]]
name = "shared_uart"
kind = "USART"
instance = 1
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/B.toml"),
        r#"name = "B"
layer = "driver"

[[stm32h5.peripherals]]
name = "shared_uart"
kind = "USART"
instance = 1
"#,
    )
    .unwrap();
    let a = run(&layout, "A");
    assert_eq!(a.related_modules.hardware_owners, vec!["B".to_string()]);
}

// ---------------------------------------------------------------------------
// 7 — library-api: items[] re_export targets populate re_export_targets.
// ---------------------------------------------------------------------------
#[test]
fn library_re_exports_populate_re_export_targets() {
    let (target, layout) = fresh_library_project("re-exports");
    std::fs::write(
        target.join("specs/modules/Inner.toml"),
        r#"name = "Inner"
visibility = "pub"
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Lib.toml"),
        r#"name = "Lib"
visibility = "pub"

[[items]]
kind = "re_export"
target = "Inner::Foo"
visibility = "pub"
"#,
    )
    .unwrap();
    let lib = run(&layout, "Lib");
    assert_eq!(lib.related_modules.re_export_targets, vec!["Inner".to_string()]);
    assert!(lib
        .related_files
        .contains(&"specs/modules/Inner.toml".to_string()));
    // library-api projects don't get the firmware-layered hardware section.
    assert!(!lib.profile_specific.contains_key("layer"));
    assert!(lib.profile_specific.contains_key("visibility"));
}

// ---------------------------------------------------------------------------
// 8 — excluded_module_count counts unrelated siblings.
// ---------------------------------------------------------------------------
#[test]
fn excluded_module_count_excludes_self_and_related() {
    let (target, layout) = fresh_firmware_project("excluded");
    // Five modules: Mid depends on Lower; the other three are unrelated.
    std::fs::write(
        target.join("specs/modules/Lower.toml"),
        "name = \"Lower\"\nlayer = \"driver\"\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Mid.toml"),
        "name = \"Mid\"\nlayer = \"middleware\"\ndepends_on = [\"Lower\"]\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/UnrelatedA.toml"),
        "name = \"UnrelatedA\"\nlayer = \"application\"\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/UnrelatedB.toml"),
        "name = \"UnrelatedB\"\nlayer = \"application\"\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/UnrelatedC.toml"),
        "name = \"UnrelatedC\"\nlayer = \"application\"\n",
    )
    .unwrap();

    let mid = run(&layout, "Mid");
    // 5 modules total; 1 related (Lower); minus M itself = 3 excluded.
    assert_eq!(mid.excluded_module_count, 3);
}

// ---------------------------------------------------------------------------
// 9 — YAML output is byte-stable across runs.
// ---------------------------------------------------------------------------
#[test]
fn context_yaml_byte_stable_across_runs() {
    let (target, layout) = fresh_firmware_project("stable");
    std::fs::write(
        target.join("specs/modules/Lower.toml"),
        "name = \"Lower\"\nlayer = \"driver\"\n",
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Mid.toml"),
        "name = \"Mid\"\nlayer = \"middleware\"\ndepends_on = [\"Lower\"]\n",
    )
    .unwrap();
    let a = context::to_yaml(&run(&layout, "Mid")).unwrap();
    let b = context::to_yaml(&run(&layout, "Mid")).unwrap();
    let c = context::to_yaml(&run(&layout, "Mid")).unwrap();
    assert_eq!(a, b);
    assert_eq!(b, c);
}

// ---------------------------------------------------------------------------
// 10 — files.schema lists inlined block + bridge schemas (lex-sorted).
// ---------------------------------------------------------------------------
#[test]
fn files_schema_lists_inlined_blocks_and_bridges() {
    let (target, layout) = fresh_firmware_project("schemas");
    run_inline_block(&layout, &phase6_plugin(), "embassy").unwrap();
    run_inline_block(&layout, &phase6_plugin(), "stm32h5-rs").unwrap();
    run_inline_bridge(&layout, &phase6_plugin(), "embassy+stm32h5-rs").unwrap();
    std::fs::write(
        target.join("specs/modules/Plain.toml"),
        "name = \"Plain\"\nlayer = \"application\"\n",
    )
    .unwrap();
    let r = run(&layout, "Plain");
    let sch = r.files.schema.join("\n");
    assert!(
        sch.contains("inlined/blocks/embassy.schema.yaml"),
        "schema list missing embassy: {sch}"
    );
    assert!(
        sch.contains("inlined/blocks/stm32h5-rs.schema.yaml"),
        "schema list missing stm32h5-rs: {sch}"
    );
    assert!(
        sch.contains("inlined/bridges/embassy+stm32h5-rs.schema.yaml"),
        "schema list missing bridge: {sch}"
    );
    // block_readmes + bridge_readmes carry plugin-root paths.
    assert!(r
        .files
        .block_readmes
        .iter()
        .any(|p| p.contains("blocks/embassy/README.md")));
    assert!(r
        .files
        .bridge_readmes
        .iter()
        .any(|p| p.contains("bridges/embassy+stm32h5-rs/README.md")));
}
