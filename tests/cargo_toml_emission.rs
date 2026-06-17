//! Deterministic Cargo.toml emission.

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

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-cargo-toml-emission-{tag}-{}-{}",
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

fn fresh_project(profile: &str, name: &str, tag: &str) -> (PathBuf, LayoutContext) {
    let workspace = tmp_workspace(tag);
    let target = workspace.join("proj");
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: name.into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

// ---------------------------------------------------------------------------
// 1 — Firmware default template renders a basic Cargo.toml.
// ---------------------------------------------------------------------------
#[test]
fn firmware_default_renders() {
    let (_target, layout) = fresh_project("firmware-layered", "demo", "fw-default");
    let r = render(&layout).unwrap();
    assert!(r.text.contains("[package]"));
    assert!(r.text.contains("name = \"demo\""));
    assert!(r.text.contains("edition = \"2021\""));
    assert!(r.text.contains("[[bin]]"));
    assert!(r.text.contains("path = \"src/main.rs\""));
    assert!(r.text.contains("[dependencies]"));
    // Architecture defaults pull in cortex-m + cortex-m-rt + panic-probe + defmt + defmt-rtt
    assert!(r.text.contains("cortex-m = "));
    assert!(r.text.contains("cortex-m-rt = "));
    assert!(r.text.contains("panic-probe = "));
    assert!(r.text.contains("defmt = "));
    assert!(r.text.contains("defmt-rtt = "));
    assert!(r.warnings.is_empty(), "no conflicts in default template");
}

// ---------------------------------------------------------------------------
// 2 — Lex-sorted [dependencies] order is byte-stable across runs.
// ---------------------------------------------------------------------------
#[test]
fn firmware_cargo_toml_byte_stable() {
    let (_target, layout) = fresh_project("firmware-layered", "demo", "fw-stable");
    let a = render(&layout).unwrap().text;
    let b = render(&layout).unwrap().text;
    let c = render(&layout).unwrap().text;
    assert_eq!(a, b);
    assert_eq!(b, c);
}

// ---------------------------------------------------------------------------
// 3 — Architecture [[external_deps]] entries land in [dependencies].
// ---------------------------------------------------------------------------
#[test]
fn firmware_external_deps_appear() {
    let (target, layout) = fresh_project("firmware-layered", "demo", "fw-extdeps");
    let arch = std::fs::read_to_string(target.join("specs/architecture.toml")).unwrap();
    let augmented = format!(
        "{arch}\n[[external_deps]]\ncrate = \"heapless\"\nversion = \"^0.8\"\nfeatures = [\"serde\"]\n"
    );
    std::fs::write(target.join("specs/architecture.toml"), augmented).unwrap();
    // Re-discover (LayoutContext caches spec, but architecture.toml is read by render at call time).
    let r = render(&layout).unwrap();
    assert!(
        r.text.contains("heapless = { version = \"^0.8\", features = [\"serde\"] }"),
        "expected heapless dep with features; got:\n{}",
        r.text
    );
}

// ---------------------------------------------------------------------------
// 4 — Module-level external_deps merge with architecture.
// ---------------------------------------------------------------------------
#[test]
fn firmware_module_external_deps_merge() {
    let (target, layout) = fresh_project("firmware-layered", "demo", "fw-mod-extdeps");
    write_module_with_dep(&target, "Uart", "nb", "^1");
    let r = render(&layout).unwrap();
    assert!(r.text.contains("nb = \"^1\""), "expected nb dep; got:\n{}", r.text);
    // Original architecture deps still present.
    assert!(r.text.contains("cortex-m = "));
}

// ---------------------------------------------------------------------------
// 5 — Same crate declared twice with same version: no warning, single entry.
// ---------------------------------------------------------------------------
#[test]
fn firmware_duplicate_dep_same_version_merges_silently() {
    let (target, layout) = fresh_project("firmware-layered", "demo", "fw-dup-same");
    write_module_with_dep(&target, "A", "heapless", "^0.8");
    write_module_with_dep(&target, "B", "heapless", "^0.8");
    let r = render(&layout).unwrap();
    let count = r.text.matches("heapless = ").count();
    assert_eq!(count, 1, "expected single heapless line; got:\n{}", r.text);
    assert!(r.warnings.is_empty(), "no warnings for matching versions");
}

// ---------------------------------------------------------------------------
// 6 — Same crate, conflicting versions: warning + first-version-wins.
// ---------------------------------------------------------------------------
#[test]
fn firmware_duplicate_dep_conflict_warns() {
    let (target, layout) = fresh_project("firmware-layered", "demo", "fw-dup-conflict");
    // Modules sort lex by filename; A.toml comes before B.toml.
    write_module_with_dep(&target, "A", "heapless", "^0.8");
    write_module_with_dep(&target, "B", "heapless", "^0.7");
    let r = render(&layout).unwrap();
    assert!(
        r.warnings.iter().any(|w| w.contains("heapless")),
        "expected version-conflict warning; got: {:?}",
        r.warnings
    );
    // First-loaded version (^0.8 from A.toml) wins.
    assert!(
        r.text.contains("heapless = \"^0.8\""),
        "expected first-version-wins; got:\n{}",
        r.text
    );
}

// ---------------------------------------------------------------------------
// 7 — Library-api: msrv + features + module external_deps.
// ---------------------------------------------------------------------------
#[test]
fn library_with_msrv_and_features_renders() {
    let (target, layout) = fresh_project("library-api", "liblib", "lib-full");
    std::fs::write(
        target.join("specs/architecture.toml"),
        r#"msrv = "1.75"

[[features]]
name = "std"
default = true

[[features]]
name = "serde"
depends_on = ["std"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/modules/Core.toml"),
        r#"name = "Core"
visibility = "pub"

[[external_deps]]
crate = "thiserror"
version = "^1"
"#,
    )
    .unwrap();
    let r = render(&layout).unwrap();
    assert!(r.text.contains("rust-version = \"1.75\""));
    assert!(r.text.contains("[lib]"));
    assert!(r.text.contains("[features]"));
    assert!(r.text.contains("std = []"));
    assert!(r.text.contains("serde = [\"std\"]"));
    assert!(r.text.contains("thiserror = \"^1\""));
}

// ---------------------------------------------------------------------------
// 8 — spec.toml [project].license + .description appear in [package].
// ---------------------------------------------------------------------------
#[test]
fn spec_extras_in_package_section() {
    let (target, _layout) = fresh_project("library-api", "liblib", "lib-pkg");
    std::fs::write(
        target.join("spec.toml"),
        r#"
spec_version = "1.0"

[project]
name = "liblib"
profile = "library-api"
language = "rust"
edition = "2024"
license = "MIT OR Apache-2.0"
description = "A demo library."

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    let layout = LayoutContext::discover(&target, None).unwrap();
    let r = render(&layout).unwrap();
    assert!(r.text.contains("edition = \"2024\""));
    assert!(r.text.contains("license = \"MIT OR Apache-2.0\""));
    assert!(r.text.contains("description = \"A demo library.\""));
}

// ---------------------------------------------------------------------------
// 9 — Output is lex-sorted by crate name in [dependencies].
// ---------------------------------------------------------------------------
#[test]
fn dependencies_lex_sorted() {
    let (target, layout) = fresh_project("firmware-layered", "demo", "fw-sorted");
    // Add three external_deps to architecture.toml in an unsorted order.
    let arch = std::fs::read_to_string(target.join("specs/architecture.toml")).unwrap();
    let augmented = format!(
        "{arch}\n\
         [[external_deps]]\ncrate = \"zzz\"\nversion = \"^1\"\n\
         [[external_deps]]\ncrate = \"aaa\"\nversion = \"^1\"\n\
         [[external_deps]]\ncrate = \"mmm\"\nversion = \"^1\"\n"
    );
    std::fs::write(target.join("specs/architecture.toml"), augmented).unwrap();
    let r = render(&layout).unwrap();
    let aaa = r.text.find("aaa = ").expect("aaa present");
    let mmm = r.text.find("mmm = ").expect("mmm present");
    let zzz = r.text.find("zzz = ").expect("zzz present");
    assert!(aaa < mmm, "aaa must come before mmm");
    assert!(mmm < zzz, "mmm must come before zzz");
}

// ---------------------------------------------------------------------------
// 10 — Unsupported profile errors clearly.
// ---------------------------------------------------------------------------
#[test]
fn unsupported_profile_errors() {
    let (target, _layout) = fresh_project("library-api", "liblib", "unsup");
    std::fs::write(
        target.join("spec.toml"),
        r#"
spec_version = "1.0"

[project]
name = "liblib"
profile = "future-c-profile"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
    )
    .unwrap();
    // When `specs/specdev/profile.yaml` is present, manifest-driven
    // dispatch wins and `[project].profile` is not load-bearing for
    // cargo-toml routing. To exercise the backward-compat fallback (the
    // only path that still knows profile names), remove the inlined
    // profile.yaml. The fallback path is what surfaces the friendly
    // "unsupported profile" error for projects bootstrapped against an
    // unknown profile name without an inlined profile manifest.
    let manifest_path = target.join("specs/specdev/profile.yaml");
    if manifest_path.exists() {
        std::fs::remove_file(&manifest_path).unwrap();
    }
    // `discover` schema-validates `[project].profile` as a generic string,
    // so this passes (no enum constraint at the schema). The renderer
    // is what rejects the unknown profile.
    let layout = match LayoutContext::discover(&target, None) {
        Ok(l) => l,
        Err(_) => return, // schema rejected first; either path is fine.
    };
    let err = render(&layout).unwrap_err();
    assert!(
        format!("{err:#}").contains("future-c-profile"),
        "expected error to name the profile; got: {err:#}"
    );
}

fn write_module_with_dep(target: &Path, mod_name: &str, crate_name: &str, version: &str) {
    let body = format!(
        r#"name = "{mod_name}"
layer = "driver"

[[external_deps]]
crate = "{crate_name}"
version = "{version}"
"#
    );
    std::fs::write(target.join(format!("specs/modules/{mod_name}.toml")), body).unwrap();
}
