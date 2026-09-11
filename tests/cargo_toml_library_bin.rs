//! `[lib]` + `[[bin]]` from a library-shaped profile.
//!
//! A library-shaped profile that declares `cargo_toml.bin` renders BOTH
//! targets — the `src/lib.rs` + `src/main.rs` crate (a testable core plus
//! a thin entry point). Library-shaped profiles that declare no `bin`
//! keep the prior `[lib]`-only behavior, which is asserted here too so
//! the widening can't silently change library-api's output.
//!
//! The `[[bin]]` name/path come from the manifest's templates (with
//! `{project_name}` substituted), and the paired stub is written once so
//! a fresh project compiles before its entry point is hand-authored.

use std::fs;
use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    cargo_toml::render,
    layout::LayoutContext,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn fresh(profile: &str, project_name: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = std::env::temp_dir().join(format!(
        "specdev-cargo-toml-library-bin-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&workspace);
    let target = workspace.join("proj");
    fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: project_name.into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

#[test]
fn rust_app_renders_lib_and_bin() {
    let (_target, layout) = fresh("rust-app", "p2p-demo", "both");
    let rendered = render(&layout).unwrap();

    assert!(
        rendered.text.contains("[lib]"),
        "rust-app must render [lib] — the testable core:\n{}",
        rendered.text
    );
    assert!(
        rendered.text.contains("[[bin]]"),
        "rust-app must render [[bin]] — the entry point:\n{}",
        rendered.text
    );
    // Templates are honored, with {project_name} substituted.
    assert!(
        rendered.text.contains("name = \"p2p-demo\""),
        "[[bin]] name comes from name_template:\n{}",
        rendered.text
    );
    assert!(
        rendered.text.contains("path = \"src/main.rs\""),
        "[[bin]] path comes from path_template:\n{}",
        rendered.text
    );
    // [lib] precedes [[bin]] — cargo doesn't care, but a stable order
    // keeps regen diffs empty.
    let lib_at = rendered.text.find("[lib]").unwrap();
    let bin_at = rendered.text.find("[[bin]]").unwrap();
    assert!(lib_at < bin_at, "[lib] should precede [[bin]]");
}

#[test]
fn rust_app_pairs_the_bin_with_a_stub_source() {
    let (_target, layout) = fresh("rust-app", "p2p-demo", "stub");
    let rendered = render(&layout).unwrap();

    let stub = rendered
        .extra_files
        .iter()
        .find(|(p, _)| p == &PathBuf::from("src/main.rs"))
        .unwrap_or_else(|| {
            panic!(
                "expected a paired src/main.rs stub, got: {:?}",
                rendered
                    .extra_files
                    .iter()
                    .map(|(p, _)| p)
                    .collect::<Vec<_>>()
            )
        });
    assert!(
        stub.1.contains("fn main()"),
        "stub body must be a compilable entry point:\n{}",
        stub.1
    );
    assert!(
        stub.1.contains("p2p-demo"),
        "stub body substitutes {{project_name}}:\n{}",
        stub.1
    );
}

/// Golden: a library-shaped profile that declares no `bin` renders
/// **byte-for-byte** what it rendered before `[[bin]]` emission existed.
///
/// Deliberately an exact-match assertion rather than a set of `contains`
/// checks. The claim the widening rests on is that library-api's output did
/// not move at all, and only byte equality states that. A containment check
/// would pass while a stray blank line or a reordered section crept in.
#[test]
fn library_profile_without_bin_is_byte_for_byte_unchanged() {
    let (_target, layout) = fresh("library-api", "mylib", "libonly");
    let rendered = render(&layout).unwrap();

    const GOLDEN: &str =
        "[package]\nname = \"mylib\"\nedition = \"2021\"\n\n[lib]\n\n[dependencies]\n";
    assert_eq!(
        rendered.text, GOLDEN,
        "library-api Cargo.toml emission moved. If this change is intended, \
         update GOLDEN — but confirm first that no `[[bin]]` leaked in from \
         the library-shape bin widening."
    );
    assert!(
        rendered.extra_files.is_empty(),
        "no paired stub without a [[bin]]: {:?}",
        rendered
            .extra_files
            .iter()
            .map(|(p, _)| p)
            .collect::<Vec<_>>()
    );
}

/// `cargo_toml.lib.crate_type` must reach the rendered manifest.
///
/// The field existed in the schema and the `LibSection` struct, but nothing
/// read it — `render_library` emitted a bare `[lib]` unconditionally. A
/// declared field with no effect validates clean and silently does nothing,
/// which is the same dead-flag class the project has hit before. It matters
/// for any library linked by a non-Rust toolchain: an FFI companion crate
/// needs `["cdylib", "staticlib"]` and got an rlib.
#[test]
fn library_crate_type_reaches_the_manifest() {
    use specdev_analyzer::manifest::Profile;

    let (target, layout) = fresh("library-api", "ffilib", "cratetype");
    let manifest_path = target.join("specs/specdev/profile.yaml");
    let original = fs::read_to_string(&manifest_path).unwrap();

    // Baseline: library-api declares `lib: {}`, so no crate-type renders.
    assert!(
        !render(&layout).unwrap().text.contains("crate-type"),
        "precondition: an empty lib section renders no crate-type"
    );

    // Declare the crate types an FFI companion crate needs.
    fs::write(
        &manifest_path,
        original.replace("  lib: {}", "  lib:\n    crate_type: [cdylib, staticlib]"),
    )
    .unwrap();
    Profile::load(&manifest_path).expect("edited manifest must still validate");

    let rendered = render(&layout).unwrap();
    assert!(
        rendered
            .text
            .contains("crate-type = [\"cdylib\", \"staticlib\"]"),
        "crate_type must render in declaration order:\n{}",
        rendered.text
    );
    // Order is the manifest's, not sorted — cargo treats the list as ordered
    // and a reordering would churn the emitted file for no reason.
    let cdylib = rendered.text.find("cdylib").unwrap();
    let staticlib = rendered.text.find("staticlib").unwrap();
    assert!(cdylib < staticlib, "declaration order preserved");
}

/// `[project].crate_type` overrides the profile's default.
///
/// Linkage is a property of the individual crate, not of the shape it shares
/// with its siblings. Most `library-api` projects want the default rlib; an
/// FFI companion crate linked from Dart needs `["cdylib", "staticlib"]`.
/// Setting that on the profile would impose the exception on every project
/// using it — the same reasoning that gave `crate_shape` a per-package
/// override.
#[test]
fn project_crate_type_overrides_the_profile_default() {
    let (target, layout) = fresh("library-api", "ffilib", "projoverride");
    let spec = target.join("spec.toml");

    // Baseline: no override, no crate-type.
    assert!(!render(&layout).unwrap().text.contains("crate-type"));

    let original = fs::read_to_string(&spec).unwrap();
    fs::write(
        &spec,
        original.replace(
            "language = \"rust\"",
            "language = \"rust\"\ncrate_type = [\"cdylib\", \"staticlib\"]",
        ),
    )
    .unwrap();
    // Re-discover so the edited spec.toml is read.
    let layout = LayoutContext::discover(&target, None).unwrap();

    let rendered = render(&layout).unwrap();
    assert!(
        rendered
            .text
            .contains("crate-type = [\"cdylib\", \"staticlib\"]"),
        "the per-project override must reach the manifest:\n{}",
        rendered.text
    );
}
