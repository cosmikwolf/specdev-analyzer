//! Phase 13-C integration tests — `gen --skeleton` emission + cargo
//! check discipline (doc 16 §2).
//!
//! Coverage:
//!   * Mode resolution: strict (no flag) routes to `/spec-gen` skill;
//!     skeleton flag emits a tree and runs cargo check.
//!   * Skeleton emission produces lib.rs + per-module .rs files with
//!     `unimplemented!()` markers.
//!   * Re-exports emit `pub use crate::<mod>::<Item>;`.
//!   * Non-library-api profile → SKEL-001.
//!   * Non-empty source dir → SKEL-002 (refuse to clobber).
//!   * `[project].handoff_mode = "skeleton"` makes default skeleton.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    layout::LayoutContext,
    skeleton,
};

fn phase6_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("phase6")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-phase13c-{tag}-{}-{}",
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

fn fresh_library_project(tag: &str, profile: &str) -> (PathBuf, LayoutContext) {
    let target = tmp_dir(tag);
    bootstrap::run_bootstrap(
        &phase6_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: "skel_demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    // Architecture.toml is profile-shipped; minimal authoring needed for
    // library-api → MSRV. Bootstrap drops a template; if minimal default
    // works, leave it; otherwise tweak below.
    let layout = LayoutContext::discover(&target, None).unwrap();
    (target, layout)
}

fn write_minimal_two_module_spec(target: &Path) {
    // Module 1: types.rs — defines a struct + an enum
    std::fs::write(
        target.join("specs/modules/Types.toml"),
        r#"name = "Types"
visibility = "pub"
description = "Public types module"

[[items]]
kind = "struct"
name = "Frame"
description = "A single frame value."
satisfies = ["REQ-LIB-1"]

[[items]]
kind = "enum"
name = "Mode"
description = "Mode discriminator."
satisfies = ["REQ-LIB-2"]
"#,
    )
    .unwrap();

    // Module 2: api.rs — has a fn + a re_export of Types::Frame
    std::fs::write(
        target.join("specs/modules/Api.toml"),
        r#"name = "Api"
visibility = "pub"
description = "Public API surface"

[[items]]
kind = "fn"
name = "compute"
description = "Compute the next frame from inputs."
satisfies = ["REQ-LIB-3"]

[[items]]
kind = "re_export"
name = "Frame"
target = "types::Frame"
visibility = "pub"
"#,
    )
    .unwrap();

    // Requirements file with the three IDs we satisfy.
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[source]]
id = "src1"
title = "ad-hoc"

[[requirement]]
id = "REQ-LIB-1"
text = "Frame type exists."
source = "src1"

[[requirement]]
id = "REQ-LIB-2"
text = "Mode enum exists."
source = "src1"

[[requirement]]
id = "REQ-LIB-3"
text = "compute fn exists."
source = "src1"
"#,
    )
    .unwrap();
}

#[test]
fn skeleton_emits_lib_and_modules_with_markers() {
    let (target, layout) = fresh_library_project("emit", "library-api");
    write_minimal_two_module_spec(&target);

    let report =
        skeleton::run_skeleton(&layout).expect("skeleton emission should succeed");

    let lib_rs = std::fs::read_to_string(target.join("src/lib.rs")).unwrap();
    assert!(lib_rs.contains("pub mod types;"));
    assert!(lib_rs.contains("pub mod api;"));

    let types_rs = std::fs::read_to_string(target.join("src/types.rs")).unwrap();
    assert!(types_rs.contains("pub struct Frame;"));
    assert!(types_rs.contains("pub enum Mode {"));

    let api_rs = std::fs::read_to_string(target.join("src/api.rs")).unwrap();
    assert!(api_rs.contains("pub fn compute() {"));
    assert!(
        api_rs.contains("unimplemented!(\"REQ-LIB-3:"),
        "expected REQ-LIB-3 marker; api.rs:\n{api_rs}"
    );
    assert!(api_rs.contains("pub use crate::types::Frame;"));

    // Cargo.toml is also emitted at project root.
    assert!(target.join("Cargo.toml").is_file());

    // Report carries module summaries.
    assert_eq!(report.modules.len(), 2);
    let api = report.modules.iter().find(|m| m.name == "Api").unwrap();
    assert_eq!(api.stub_count, 1);
    assert_eq!(api.re_export_count, 1);
}

#[test]
fn skeleton_refuses_non_library_api_profile() {
    let (target, layout) = fresh_library_project("non-libapi", "firmware-layered");
    // Provide a minimal architecture.toml so layout discovery doesn't reject
    // a no-architecture firmware project — but skeleton should bail out
    // before it ever reads architecture.
    let _ = target;

    let err = skeleton::run_skeleton(&layout).unwrap_err();
    assert!(
        format!("{err:#}").contains("SKEL-001"),
        "expected SKEL-001 finding; got {err:#}"
    );
}

#[test]
fn skeleton_refuses_non_empty_source_dir() {
    let (target, layout) = fresh_library_project("non-empty", "library-api");
    write_minimal_two_module_spec(&target);
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(target.join("src/preexisting.rs"), "// not a skeleton\n").unwrap();

    let err = skeleton::run_skeleton(&layout).unwrap_err();
    assert!(
        format!("{err:#}").contains("SKEL-002"),
        "expected SKEL-002; got {err:#}"
    );
    // The existing file must be preserved.
    assert!(target.join("src/preexisting.rs").is_file());
}

#[test]
fn cli_strict_mode_routes_to_spec_gen_skill() {
    let (target, _layout) = fresh_library_project("strict-route", "library-api");
    write_minimal_two_module_spec(&target);

    let out = Command::new(analyzer_bin())
        .arg("gen")
        .arg("--project")
        .arg(&target)
        .arg("--strict")
        .output()
        .expect("invoke gen --strict");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "strict mode should exit non-zero (it routes to the skill); out:\n{combined}");
    assert!(
        combined.contains("/spec-gen") || combined.contains("spec-gen skill"),
        "expected routing message; got:\n{combined}"
    );
}

#[test]
fn cli_handoff_mode_skeleton_makes_default_skeleton() {
    let (target, _layout) = fresh_library_project("default-skel", "library-api");
    write_minimal_two_module_spec(&target);

    // Set [project].handoff_mode = "skeleton" so plain `gen` resolves
    // to skeleton mode.
    let spec_path = target.join("spec.toml");
    let mut text = std::fs::read_to_string(&spec_path).unwrap();
    text = text.replace(
        "[project]",
        "[project]\nhandoff_mode = \"skeleton\"",
    );
    std::fs::write(&spec_path, text).unwrap();

    let out = Command::new(analyzer_bin())
        .arg("gen")
        .arg("--project")
        .arg(&target)
        .output()
        .expect("invoke gen (no flags)");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.status.success(),
        "default-skeleton run should succeed when cargo check passes; out:\n{combined}"
    );
    assert!(target.join("src/lib.rs").is_file(), "lib.rs should be emitted");
}

#[test]
fn cli_strict_and_skeleton_flags_are_mutually_exclusive() {
    let (target, _layout) = fresh_library_project("conflict", "library-api");
    write_minimal_two_module_spec(&target);

    let out = Command::new(analyzer_bin())
        .arg("gen")
        .arg("--project")
        .arg(&target)
        .arg("--strict")
        .arg("--skeleton")
        .output()
        .expect("invoke gen --strict --skeleton");
    assert!(!out.status.success(), "clap should reject conflicting flags");
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.to_lowercase().contains("cannot be used")
            || combined.to_lowercase().contains("conflict"),
        "expected conflict error; got:\n{combined}"
    );
}

#[test]
fn skeleton_marker_carries_satisfies_requirement_ids() {
    let (target, layout) = fresh_library_project("marker", "library-api");
    std::fs::write(
        target.join("specs/modules/Solo.toml"),
        r#"name = "Solo"
visibility = "pub"

[[items]]
kind = "fn"
name = "do_thing"
description = "Does the thing precisely."
satisfies = ["REQ-001", "REQ-002"]
"#,
    )
    .unwrap();
    std::fs::write(
        target.join("specs/requirements.toml"),
        r#"
[[source]]
id = "s1"
title = "spec"

[[requirement]]
id = "REQ-001"
text = "first"
source = "s1"

[[requirement]]
id = "REQ-002"
text = "second"
source = "s1"
"#,
    )
    .unwrap();

    skeleton::run_skeleton(&layout).expect("emission");
    let solo = std::fs::read_to_string(target.join("src/solo.rs")).unwrap();
    assert!(
        solo.contains("unimplemented!(\"REQ-001, REQ-002: Does the thing precisely\")"),
        "marker should carry both REQ ids and one-line desc; solo.rs:\n{solo}"
    );
}
