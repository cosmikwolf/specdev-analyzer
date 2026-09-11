//! Profile `module.schema.yaml` gains `items[]` field; substitution swaps
//! `{{language}}` for the project's language at bootstrap; validate accepts
//! module specs with items[].
//!
//! Coverage:
//!   * Bootstrap substitutes `{{language}}` in `module.schema.yaml`'s
//!     items[] $ref to the chosen language.
//!   * A library-api module with items[] entries (including a `re_export`
//!     that omits `satisfies`) validates clean.
//!   * Sparseness rule: a non-`re_export` item without `satisfies` errors.
//!   * UTF-8 safety regression: substitute() preserves multi-byte chars
//!     (em-dashes in YAML schema descriptions).

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-module-items-{tag}-{}-{}",
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

fn run_validate(project: &Path) -> (bool, String, String) {
    let out = Command::new(analyzer_bin())
        .arg("validate")
        .arg("--project")
        .arg(project)
        .arg("--no-color")
        .output()
        .expect("invoke validate");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn bootstrap_substitutes_language_in_module_schema() {
    let plugin_root = full_plugin();
    let target = tmp_dir("substitute");
    bootstrap::run_bootstrap(
        &plugin_root,
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let schema_text =
        std::fs::read_to_string(target.join("specs/specdev/schemas/module.schema.yaml")).unwrap();
    assert!(
        schema_text.contains("inlined/languages/rust/items.schema.yaml"),
        "expected items[] $ref to be substituted; schema:\n{schema_text}"
    );
    assert!(
        !schema_text.contains("{{language}}"),
        "expected no unsubstituted placeholders"
    );
}

#[test]
fn substitute_preserves_utf8_em_dash() {
    // Regression test: an earlier substitute() iterated bytes and cast
    // to char, mangling multi-byte UTF-8. The library-api/firmware-layered
    // module.schema.yaml carries em-dashes ("—") in description prose; a
    // bytes-as-chars copy would corrupt them. This test asserts the byte
    // sequence survives a round-trip.
    let plugin_root = full_plugin();
    let target = tmp_dir("utf8");
    bootstrap::run_bootstrap(
        &plugin_root,
        &BootstrapInputs {
            profile: "firmware-layered".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let bytes = std::fs::read(target.join("specs/specdev/schemas/module.schema.yaml")).unwrap();
    // Em-dash UTF-8 is 0xE2 0x80 0x94. Plain hyphen is 0x2D.
    assert!(
        bytes.windows(3).any(|w| w == [0xE2, 0x80, 0x94]),
        "em-dash bytes (0xE2 0x80 0x94) absent from substituted schema"
    );
    // The string should parse as valid UTF-8.
    let text = String::from_utf8(bytes).expect("output is valid UTF-8");
    assert!(
        text.contains("—"),
        "em-dash char absent from substituted schema"
    );
}

#[test]
fn library_module_with_items_validates_clean() {
    let plugin_root = full_plugin();
    let target = tmp_dir("items-clean");
    bootstrap::run_bootstrap(
        &plugin_root,
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    // Author one minimal module with items[].
    std::fs::write(
        target.join("specs/modules/Foo.toml"),
        r#"name = "Foo"
visibility = "pub"

[[items]]
name = "do_thing"
kind = "fn"
description = "Public entry point that does the thing the library was made for."
satisfies = ["DEMO-1"]

[[items]]
name = "Reexported"
kind = "re_export"
description = "Convenience re-export of the inner-module type."
target = "internal::Reexported"
visibility = "pub"
"#,
    )
    .unwrap();

    let (ok, _stdout, stderr) = run_validate(&target);
    assert!(
        ok,
        "validate should succeed for items[]-bearing module; stderr:\n{stderr}"
    );
}

#[test]
fn library_module_item_without_satisfies_errors() {
    let plugin_root = full_plugin();
    let target = tmp_dir("items-sparseness");
    bootstrap::run_bootstrap(
        &plugin_root,
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    // A non-re_export item without satisfies — sparseness rule violation.
    std::fs::write(
        target.join("specs/modules/Bar.toml"),
        r#"name = "Bar"
visibility = "pub"

[[items]]
name = "lonely"
kind = "fn"
description = "A function without any requirement traceability."
"#,
    )
    .unwrap();

    let (ok, _stdout, stderr) = run_validate(&target);
    assert!(
        !ok,
        "validate should fail when a non-re_export item omits satisfies; stderr:\n{stderr}"
    );
}
