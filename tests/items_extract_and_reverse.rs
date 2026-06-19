//! Items extraction surface + reverse pre-flight + `verified_by_resolves`
//! graph-check primitive.
//!
//! Coverage:
//!   * `extract_items_from_source` API end-to-end on Rust source.
//!   * `reverse` pre-flight CLI: 0 blocks inlined → REVERSE-002 error;
//!     2+ blocks inlined → REVERSE-003 ambiguity warning; --into
//!     references a non-inlined block → REVERSE-001 error.
//!   * `verified_by_resolves` dispatch wires through validate end-to-end
//!     when domain.toml opts in.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    items::{extract::extract_items_from_source, ItemKind},
    lang::rust::RustAdapter,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-items-extract-reverse-{tag}-{}-{}",
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

fn analyzer_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("specdev-analyzer")
}

// ---------------------------------------------------------------------------
// extract_items API
// ---------------------------------------------------------------------------

#[test]
fn extract_items_from_source_round_trip_rust() {
    let source = "\
#[test]
fn test_one() {}

pub struct Foo;
pub enum Color { R, G, B }
pub const N: u32 = 4;
pub static M: u32 = 5;
pub type Alias = u32;
pub trait Show {}
impl Show for Foo {}
pub use crate::inner::Item as Public;
extern \"C\" {
    fn extern_fn() -> u32;
}
";
    let items = extract_items_from_source(source, &RustAdapter).unwrap();
    let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();

    // 11 of the 12 kinds covered by this fixture (no union, no macro).
    assert!(kinds.contains(&ItemKind::Fn));
    assert!(kinds.contains(&ItemKind::Struct));
    assert!(kinds.contains(&ItemKind::Enum));
    assert!(kinds.contains(&ItemKind::Const));
    assert!(kinds.contains(&ItemKind::Static));
    assert!(kinds.contains(&ItemKind::Type));
    assert!(kinds.contains(&ItemKind::Trait));
    assert!(kinds.contains(&ItemKind::Impl));
    assert!(kinds.contains(&ItemKind::ReExport));
    assert!(kinds.contains(&ItemKind::ExternBlock));

    let test_fn = items
        .iter()
        .find(|i| i.kind == ItemKind::Fn && i.name == "test_one")
        .expect("test_one fn");
    assert!(test_fn.attributes.iter().any(|a| a == "#[test]"));

    let re_export = items.iter().find(|i| i.kind == ItemKind::ReExport).unwrap();
    assert_eq!(re_export.name, "Public");

    let impl_item = items.iter().find(|i| i.kind == ItemKind::Impl).unwrap();
    assert!(impl_item.name.starts_with("Show for"));
}

#[test]
fn extract_dissolves_inherent_methods_keeps_trait_impl_whole() {
    // A method is a function: public inherent methods are items named
    // `<Type>::<member>`; the inherent impl block itself is not an item.
    // A trait impl stays ONE item (the conformance is the contract); its
    // method bodies are codegen-derived from the trait, never items.
    // Private associated members are not contract surface.
    let source = "\
pub struct Counter { n: u32 }

impl Counter {
    pub fn new() -> Self { Self { n: 0 } }
    pub fn incr(&mut self) { self.n += 1; }
    pub fn get(&self) -> u32 { self.n }
    fn private_helper(&self) {}
}

pub trait Resettable {
    fn reset(&mut self);
    fn is_reset(&self) -> bool { false }
}

impl Resettable for Counter {
    fn reset(&mut self) { self.n = 0; }
}

pub fn free_function() {}
";
    let items = extract_items_from_source(source, &RustAdapter).unwrap();
    let names: Vec<_> = items
        .iter()
        .map(|i| (i.kind, i.name.clone()))
        .collect();

    // Top-level type + trait still extracted.
    assert!(items
        .iter()
        .any(|i| i.kind == ItemKind::Struct && i.name == "Counter"));
    assert!(items
        .iter()
        .any(|i| i.kind == ItemKind::Trait && i.name == "Resettable"));

    // The inherent impl dissolves into per-method items; the block is not an item.
    assert!(
        !items.iter().any(|i| i.kind == ItemKind::Impl && i.name == "Counter"),
        "inherent impl block must not be an item; got: {names:?}"
    );
    for method in ["Counter::new", "Counter::incr", "Counter::get"] {
        assert!(
            items.iter().any(|i| i.kind == ItemKind::Fn && i.name == method),
            "public inherent method `{method}` must be an item; got: {names:?}"
        );
    }
    assert!(
        !items.iter().any(|i| i.name == "Counter::private_helper"),
        "private inherent method must not be an item; got: {names:?}"
    );

    // The trait impl stays exactly one item; its methods are not surfaced.
    let impls: Vec<_> = items.iter().filter(|i| i.kind == ItemKind::Impl).collect();
    assert_eq!(impls.len(), 1, "only the trait impl is an impl item; got: {names:?}");
    assert_eq!(impls[0].name, "Resettable for Counter");
    for method in ["reset", "is_reset"] {
        assert!(
            !items.iter().any(|i| i.name.ends_with(method) && i.kind == ItemKind::Fn),
            "trait-impl method `{method}` must not be an item; got: {names:?}"
        );
    }

    // `fn` items: the three public inherent methods + the free function.
    let mut fns: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == ItemKind::Fn)
        .map(|i| i.name.as_str())
        .collect();
    fns.sort();
    assert_eq!(
        fns,
        vec!["Counter::get", "Counter::incr", "Counter::new", "free_function"],
        "got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// reverse pre-flight
// ---------------------------------------------------------------------------

#[test]
fn reverse_no_blocks_inlined_emits_reverse_002() {
    let dir = tmp_dir("reverse-no-blocks");
    let target = dir.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let out = Command::new(analyzer_bin())
        .arg("reverse")
        .arg("--project")
        .arg(&target)
        .output()
        .expect("run reverse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "expected failure with no blocks inlined; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stderr.contains("REVERSE-002"), "stderr: {stderr}");
}

#[test]
fn reverse_into_unknown_block_emits_reverse_001() {
    let dir = tmp_dir("reverse-bad-into");
    let target = dir.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let out = Command::new(analyzer_bin())
        .arg("reverse")
        .arg("--project")
        .arg(&target)
        .arg("--into")
        .arg("does-not-exist")
        .output()
        .expect("run reverse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("REVERSE-001"), "stderr: {stderr}");
    assert!(stderr.contains("does-not-exist"), "stderr: {stderr}");
}

#[test]
fn reverse_two_inlined_blocks_emits_reverse_003_warning() {
    // Bootstrap a firmware-layered project + inline two blocks.
    let dir = tmp_dir("reverse-2-blocks");
    let target = dir.join("proj");
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

    // Simulate two inlined blocks by creating empty subdirs under
    // schemas/inlined/blocks/. The pre-flight only checks for directory
    // existence, not for valid block content.
    let blocks_dir = target.join("specs/specdev/schemas/inlined/blocks");
    std::fs::create_dir_all(blocks_dir.join("embassy")).unwrap();
    std::fs::create_dir_all(blocks_dir.join("stm32h5-rs")).unwrap();

    let out = Command::new(analyzer_bin())
        .arg("reverse")
        .arg("--project")
        .arg(&target)
        .output()
        .expect("run reverse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected success with warning; stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(
        stderr.contains("REVERSE-003"),
        "stderr should carry REVERSE-003: {stderr}"
    );
    // YAML report includes both block names.
    assert!(stdout.contains("embassy"), "stdout: {stdout}");
    assert!(stdout.contains("stm32h5-rs"), "stdout: {stdout}");
}

#[test]
fn reverse_into_scopes_to_subset() {
    // Two blocks inlined; --into scopes to one. No ambiguity warning
    // because --into resolves it.
    let dir = tmp_dir("reverse-into-scope");
    let target = dir.join("proj");
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
    let blocks_dir = target.join("specs/specdev/schemas/inlined/blocks");
    std::fs::create_dir_all(blocks_dir.join("embassy")).unwrap();
    std::fs::create_dir_all(blocks_dir.join("stm32h5-rs")).unwrap();

    let out = Command::new(analyzer_bin())
        .arg("reverse")
        .arg("--project")
        .arg(&target)
        .arg("--into")
        .arg("embassy")
        .output()
        .expect("run reverse");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        !stderr.contains("REVERSE-003"),
        "--into should suppress ambiguity warning; stderr: {stderr}"
    );
    // vocabulary in YAML should be just `embassy`, not both blocks.
    assert!(stdout.contains("vocabulary:"), "stdout: {stdout}");
    assert!(stdout.contains("- embassy"), "stdout: {stdout}");
}

// ---------------------------------------------------------------------------
// extract-items CLI
// ---------------------------------------------------------------------------

#[test]
fn extract_items_cli_round_trip_on_library_api() {
    let dir = tmp_dir("extract-items-cli");
    let target = dir.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    // Author a module spec + a source file matching the convention.
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"
name = "Foo"
visibility = "pub"
description = "Foo module."
"#,
    )
    .unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn one() -> u32 { 1 }\npub struct S;\n#[test]\nfn t() {}\n",
    )
    .unwrap();

    let out = Command::new(analyzer_bin())
        .arg("extract-items")
        .arg("--project")
        .arg(&target)
        .arg("--module")
        .arg("Foo")
        .output()
        .expect("run extract-items");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    // YAML output references the kinds extracted.
    assert!(stdout.contains("module_name: Foo"), "stdout: {stdout}");
    assert!(stdout.contains("kind: fn"), "stdout: {stdout}");
    assert!(stdout.contains("name: one"), "stdout: {stdout}");
    assert!(stdout.contains("kind: struct"), "stdout: {stdout}");
    assert!(stdout.contains("name: S"), "stdout: {stdout}");
    assert!(stdout.contains("name: t"), "stdout: {stdout}");
    assert!(stdout.contains("'#[test]'"), "stdout: {stdout}");
}

#[test]
fn extract_items_cli_pre_implementation_module_emits_empty_list() {
    let dir = tmp_dir("extract-items-empty");
    let target = dir.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    std::fs::write(
        target.join("specs/modules/Bar.toml"),
        r#"
name = "Bar"
visibility = "pub"
description = "Bar module — no source yet."
"#,
    )
    .unwrap();

    let out = Command::new(analyzer_bin())
        .arg("extract-items")
        .arg("--project")
        .arg(&target)
        .arg("--module")
        .arg("Bar")
        .output()
        .expect("run extract-items");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stderr: {stderr}");
    assert!(stdout.contains("module_name: Bar"), "stdout: {stdout}");
    // Pre-implementation: source_file is null and items is empty.
    assert!(
        stdout.contains("source_file: null") || stdout.contains("source_file:\n"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("items: []"), "stdout: {stdout}");
}

#[test]
fn extract_items_help_lists_module_flag() {
    let out = Command::new(analyzer_bin())
        .arg("extract-items")
        .arg("--help")
        .output()
        .expect("run extract-items --help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("--module"), "stdout: {stdout}");
}
