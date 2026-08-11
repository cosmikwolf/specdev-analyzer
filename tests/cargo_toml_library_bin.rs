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
                rendered.extra_files.iter().map(|(p, _)| p).collect::<Vec<_>>()
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

    const GOLDEN: &str = "[package]\nname = \"mylib\"\nedition = \"2021\"\n\n[lib]\n\n[dependencies]\n";
    assert_eq!(
        rendered.text, GOLDEN,
        "library-api Cargo.toml emission moved. If this change is intended, \
         update GOLDEN — but confirm first that no `[[bin]]` leaked in from \
         the library-shape bin widening."
    );
    assert!(
        rendered.extra_files.is_empty(),
        "no paired stub without a [[bin]]: {:?}",
        rendered.extra_files.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );
}
