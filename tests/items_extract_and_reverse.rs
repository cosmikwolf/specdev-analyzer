//! Items extraction surface + reverse pre-flight.
//!
//! Coverage:
//!   * `extract_items_from_source` API end-to-end on Rust source.
//!   * `reverse` pre-flight CLI: 0 blocks inlined → REVERSE-002 error;
//!     2+ blocks inlined → REVERSE-003 ambiguity warning; --into
//!     references a non-inlined block → REVERSE-001 error.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline,
    items::extract::extract_items_from_source,
    lang::rust::RustAdapter,
    layout::LayoutContext,
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
pub fn test_one() {}

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
    let items = extract_items_from_source(source, "rs", &RustAdapter, None).unwrap();
    let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();

    // 11 of the 12 kinds covered by this fixture (no union, no macro).
    assert!(kinds.contains(&"fn"));
    assert!(kinds.contains(&"struct"));
    assert!(kinds.contains(&"enum"));
    assert!(kinds.contains(&"const"));
    assert!(kinds.contains(&"static"));
    assert!(kinds.contains(&"type"));
    assert!(kinds.contains(&"trait"));
    assert!(kinds.contains(&"impl"));
    assert!(kinds.contains(&"re_export"));
    assert!(kinds.contains(&"extern_block"));

    let test_fn = items
        .iter()
        .find(|i| i.kind == "fn" && i.name == "test_one")
        .expect("test_one fn");
    assert!(test_fn.attributes.iter().any(|a| a == "#[test]"));

    let re_export = items.iter().find(|i| i.kind == "re_export").unwrap();
    assert_eq!(re_export.name, "Public");

    let impl_item = items.iter().find(|i| i.kind == "impl").unwrap();
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
    let items = extract_items_from_source(source, "rs", &RustAdapter, None).unwrap();
    let names: Vec<_> = items
        .iter()
        .map(|i| (i.kind.clone(), i.name.clone()))
        .collect();

    // Top-level type + trait still extracted.
    assert!(items
        .iter()
        .any(|i| i.kind == "struct" && i.name == "Counter"));
    assert!(items
        .iter()
        .any(|i| i.kind == "trait" && i.name == "Resettable"));

    // The inherent impl dissolves into per-method items; the block is not an item.
    assert!(
        !items
            .iter()
            .any(|i| i.kind == "impl" && i.name == "Counter"),
        "inherent impl block must not be an item; got: {names:?}"
    );
    for method in ["Counter::new", "Counter::incr", "Counter::get"] {
        assert!(
            items.iter().any(|i| i.kind == "fn" && i.name == method),
            "public inherent method `{method}` must be an item; got: {names:?}"
        );
    }
    assert!(
        !items.iter().any(|i| i.name == "Counter::private_helper"),
        "private inherent method must not be an item; got: {names:?}"
    );

    // The trait impl stays exactly one item; its methods are not surfaced.
    let impls: Vec<_> = items.iter().filter(|i| i.kind == "impl").collect();
    assert_eq!(
        impls.len(),
        1,
        "only the trait impl is an impl item; got: {names:?}"
    );
    assert_eq!(impls[0].name, "Resettable for Counter");
    for method in ["reset", "is_reset"] {
        assert!(
            !items
                .iter()
                .any(|i| i.name.ends_with(method) && i.kind == "fn"),
            "trait-impl method `{method}` must not be an item; got: {names:?}"
        );
    }

    // `fn` items: the three public inherent methods + the free function.
    let mut fns: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == "fn")
        .map(|i| i.name.as_str())
        .collect();
    fns.sort();
    assert_eq!(
        fns,
        vec![
            "Counter::get",
            "Counter::incr",
            "Counter::new",
            "free_function"
        ],
        "got: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// reverse pre-flight
// ---------------------------------------------------------------------------

// A profile WITHOUT `reverse.vocabulary: profile` (firmware-layered) still
// stops on REVERSE-002 when nothing is inlined.
#[test]
fn reverse_no_blocks_inlined_emits_reverse_002() {
    let dir = tmp_dir("reverse-no-blocks");
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

// #102: library-api declares `reverse.vocabulary: profile`, so zero inlined
// blocks is REVERSE-004 (info) and the run succeeds with the profile as
// the vocabulary. Keyed on the declaration the fixture profile.yaml
// carries, not on the profile name.
#[test]
fn reverse_profile_vocabulary_emits_reverse_004_and_succeeds() {
    let dir = tmp_dir("reverse-profile-vocab");
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
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("REVERSE-004"), "stdout: {stdout}");
    assert!(stdout.contains("- profile:library-api"), "stdout: {stdout}");
    assert!(!stderr.contains("REVERSE-002"), "stderr: {stderr}");
}

/// Every profile that needs no block to describe a module declares
/// `reverse.vocabulary: profile`, so a fresh tree passes the pre-flight with
/// REVERSE-004. The profiles whose modules name block fields (firmware,
/// audio plugin, workspace root) still need a block and are not listed.
const PROFILE_VOCABULARY: &[(&str, &str)] = &[
    ("library-api", "rust"),
    ("rust-app", "rust"),
    ("nextjs-app", "typescript"),
    ("ts-library", "typescript"),
    ("dart-package", "dart"),
    ("flutter-app", "dart"),
    ("python-library", "python"),
    ("python-ffi", "python"),
];

fn bootstrapped(tag: &str, profile: &str, language: &str) -> PathBuf {
    let target = tmp_dir(tag).join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: language.into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap_or_else(|e| panic!("bootstrap {profile}: {e}"));
    target
}

#[test]
fn every_blockless_profile_passes_the_reverse_preflight() {
    for (profile, language) in PROFILE_VOCABULARY {
        let target = bootstrapped(&format!("vocab-{profile}"), profile, language);
        let out = Command::new(analyzer_bin())
            .arg("reverse")
            .arg("--project")
            .arg(&target)
            .output()
            .expect("run reverse");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success() && stdout.contains("REVERSE-004"),
            "{profile}: stdout: {stdout}\nstderr: {stderr}"
        );
    }
}

/// A requirement's `source` ids resolve to `[[source]]` entries in every
/// profile that has requirements: an unknown id is an error, a known one is
/// not.
#[test]
fn a_requirement_source_id_must_resolve_in_every_profile() {
    let profiles = PROFILE_VOCABULARY
        .iter()
        .copied()
        .chain([("firmware-layered", "rust"), ("audio-plugin", "rust")]);
    for (profile, language) in profiles {
        let target = bootstrapped(&format!("source-{profile}"), profile, language);
        std::fs::create_dir_all(target.join("specs/requirements")).unwrap();
        std::fs::create_dir_all(target.join("specs/sources")).unwrap();
        std::fs::write(
            target.join("specs/sources/read-from-code.toml"),
            "[[source]]\nid = \"read-from-code\"\nkind = \"derived\"\ntitle = \"Read from the code\"\n",
        )
        .unwrap();
        let req = |id: &str, source: &str| {
            format!(
                "[[requirement]]\nid = \"{id}\"\ntext = \"The system shall do {id}.\"\nnature = \"module\"\ntest_pending = true\nsource = [\"{source}\"]\n"
            )
        };
        std::fs::write(
            target.join("specs/requirements/REQ-SRC-001.toml"),
            req("REQ-SRC-001", "read-from-code"),
        )
        .unwrap();
        std::fs::write(
            target.join("specs/requirements/REQ-SRC-002.toml"),
            req("REQ-SRC-002", "no-such-source"),
        )
        .unwrap();
        let out = Command::new(analyzer_bin())
            .arg("validate")
            .arg("--no-color")
            .current_dir(&target)
            .output()
            .expect("run validate");
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let unresolved: Vec<&str> = text
            .lines()
            .filter(|l| l.contains("REDUCE_UNRESOLVED"))
            .collect();
        assert!(
            unresolved.iter().any(|l| l.contains("no-such-source")),
            "{profile}: unknown source id not reported:\n{text}"
        );
        assert!(
            !unresolved.iter().any(|l| l.contains("read-from-code")),
            "{profile}: a defined source id was reported:\n{text}"
        );
    }
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

    // Inline two real blocks so the pre-flight reads what `inline` actually
    // writes (lineage.toml + flat `<block>.schema.yaml` files), not a
    // simulated layout.
    let layout = LayoutContext::discover(&target, None).unwrap();
    inline::run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    inline::run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();

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
    let layout = LayoutContext::discover(&target, None).unwrap();
    inline::run_inline_block(&layout, &full_plugin(), "embassy").unwrap();
    inline::run_inline_block(&layout, &full_plugin(), "stm32h5-rs").unwrap();

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
    // `make_bucket` is a private (non-`pub`) free helper: per R-002 it is
    // module-private, not contract surface, so extraction must filter it.
    std::fs::write(
        target.join("src/foo.rs"),
        "pub fn one() -> u32 { 1 }\npub struct S;\nfn make_bucket() -> u32 { 2 }\n",
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
    // The private free fn must NOT surface (R-002).
    assert!(
        !stdout.contains("name: make_bucket"),
        "private free fn must be filtered; stdout: {stdout}"
    );
}

// #101(a) + #101(b) end to end: firmware-layered's fixture profile.yaml
// declares `items.visibility_floor: "pub(crate)"`; the module spec names a
// nested task-body file through `source_file`. A `pub(crate) fn` and a
// `pub(crate) const` extract, a private const does not (#103), and a
// `pub(crate) use` stays because the spec's re_export enum admits `crate`
// (#104).
#[test]
fn extract_items_cli_honours_source_file_override_and_profile_floor() {
    let dir = tmp_dir("extract-items-floor");
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

    std::fs::write(
        target.join("specs/modules/Blink.toml"),
        r#"
name = "Blink"
layer = "application"
description = "Blink task body."
source_file = "src/tasks/blink.rs"
"#,
    )
    .unwrap();
    std::fs::create_dir_all(target.join("src/tasks")).unwrap();
    // The convention file exists too and must NOT be the one read.
    std::fs::write(target.join("src/blink.rs"), "pub fn decoy() {}\n").unwrap();
    std::fs::write(
        target.join("src/tasks/blink.rs"),
        "pub(crate) fn init_peripherals() {}\npub(crate) const DEBOUNCE_TICKS: u8 = 3;\n\
         const PRIVATE_TICKS: u8 = 4;\npub(super) fn narrower() {}\n\
         pub(crate) use crate::hal::Led;\n",
    )
    .unwrap();

    let out = Command::new(analyzer_bin())
        .arg("extract-items")
        .arg("--project")
        .arg(&target)
        .arg("--module")
        .arg("Blink")
        .output()
        .expect("run extract-items");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("src/tasks/blink.rs"), "stdout: {stdout}");
    assert!(
        !stdout.contains("decoy"),
        "override must win over the convention file; stdout: {stdout}"
    );
    assert!(
        stdout.contains("name: init_peripherals"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("name: DEBOUNCE_TICKS"), "stdout: {stdout}");
    assert!(stdout.contains("name: Led"), "stdout: {stdout}");
    assert!(
        !stdout.contains("PRIVATE_TICKS"),
        "private const must drop; stdout: {stdout}"
    );
    assert!(
        !stdout.contains("narrower"),
        "pub(super) is below a pub(crate) floor; stdout: {stdout}"
    );
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
