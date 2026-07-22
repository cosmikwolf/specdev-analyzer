//! Byte-level snapshot guard over the Rust `extract-items` and
//! `items diff` CLI surface.
//!
//! The items internals are refactor-prone (kind representation, identity
//! normalization, adapter dispatch). This test pins the exact CLI output
//! on a fixture that exercises every Rust item kind plus the identity
//! edge cases (generic impls, path-qualified traits, inherent members,
//! visibility filtering, test-item exclusion, signature drift), so any
//! internal refactor that changes observable Rust behavior fails here
//! byte-for-byte rather than silently.

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
        "specdev-items-snapshot-{tag}-{}-{}",
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

fn run_cli(project: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(analyzer_bin())
        .args(args)
        .arg("--project")
        .arg(project)
        .output()
        .expect("invoke analyzer");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Replace the tmp project root with a stable token so snapshots are
/// machine-independent.
fn normalize(text: &str, root: &Path) -> String {
    text.replace(&root.display().to_string(), "<root>")
}

const MODULE_SPEC: &str = r#"name = "Kitchen"
visibility = "pub"

[[items]]
name = "render"
kind = "fn"
description = "Render the frame."
satisfies = ["DEMO-1"]
signature_hint = "fn render() -> u32"

[[items]]
name = "Widget"
kind = "struct"
description = "A widget."
satisfies = ["DEMO-1"]

[[items]]
name = "Color"
kind = "enum"
description = "A color."
satisfies = ["DEMO-1"]

[[items]]
name = "Packed"
kind = "union"
description = "A packed union."
satisfies = ["DEMO-1"]

[[items]]
name = "Show"
kind = "trait"
description = "Display-ish trait."
satisfies = ["DEMO-1"]

[[items]]
name = "Display for Widget"
kind = "impl"
description = "Widget renders as text."
satisfies = ["DEMO-1"]
target_type = "Widget"
trait_ref = "core::fmt::Display"

[[items]]
name = "Widget::grow"
kind = "fn"
description = "Grow the widget."
satisfies = ["DEMO-1"]

[[items]]
name = "Widget::MAX"
kind = "const"
description = "Maximum widget size."
satisfies = ["DEMO-1"]

[[items]]
name = "Widget::Out"
kind = "type"
description = "Widget output type."
satisfies = ["DEMO-1"]

[[items]]
name = "Alias"
kind = "type"
description = "A type alias."
satisfies = ["DEMO-1"]

[[items]]
name = "LIMIT"
kind = "const"
description = "A const."
satisfies = ["DEMO-1"]

[[items]]
name = "COUNTER"
kind = "static"
description = "A static."
satisfies = ["DEMO-1"]

[[items]]
name = "Public"
kind = "re_export"
description = "Re-exported inner item."
target = "inner::Item"
visibility = "pub"
alias = "Public"

[[items]]
name = "make_widget"
kind = "macro"
description = "Widget construction macro."
satisfies = ["DEMO-1"]

[[items]]
name = "extern_C_at_40"
kind = "extern_block"
description = "FFI surface."
satisfies = ["DEMO-1"]

[[items]]
name = "ghost"
kind = "fn"
description = "Spec-only item, missing from source."
satisfies = ["DEMO-1"]

[[items]]
name = "drifted"
kind = "fn"
description = "Signature hint diverges from source."
satisfies = ["DEMO-1"]
signature_hint = "fn drifted(x: u8) -> u8"

[[items]]
name = "render_is_idempotent"
kind = "fn"
description = "Unit test for render."
test = true
test_kind = "unit"
verifies = ["DEMO-1"]
"#;

const SOURCE: &str = r#"pub fn render() -> u32 { 0 }

fn private_helper() -> u32 { 1 }

pub fn uncovered() -> u32 { 2 }

pub fn drifted(x: u16) -> u16 { x }

pub struct Widget;

pub enum Color { R, G, B }

pub union Packed { a: u32, b: f32 }

pub trait Show {}

impl core::fmt::Display for Widget {
    fn fmt(&self, _f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        Ok(())
    }
}

impl Widget {
    pub fn grow(&mut self) {}
    fn shrink(&mut self) {}
    pub const MAX: u32 = 16;
    pub type Out = u32;
}

pub type Alias = u32;

pub const LIMIT: u32 = 8;

pub static COUNTER: u32 = 0;

pub use inner::Item as Public;

pub(crate) use inner::Hidden;

macro_rules! make_widget {
    () => {};
}

extern "C" {
    fn ffi_fn() -> u32;
}

#[cfg(test)]
mod tests {
    #[test]
    fn render_is_idempotent() {}
}
"#;

fn build_project(tag: &str) -> PathBuf {
    let target = tmp_dir(tag);
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
    std::fs::write(target.join("specs/modules/Kitchen.toml"), MODULE_SPEC).unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(target.join("src/kitchen.rs"), SOURCE).unwrap();
    target
}

#[test]
fn extract_items_output_is_stable() {
    let target = build_project("extract");
    let (code, stdout, stderr) = run_cli(&target, &["extract-items", "--module", "Kitchen"]);
    let rendered = format!(
        "exit: {code:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        normalize(&stdout, &target),
        normalize(&stderr, &target),
    );
    insta::assert_snapshot!(rendered);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn items_diff_output_is_stable() {
    let target = build_project("diff");
    let (code, stdout, stderr) = run_cli(&target, &["items", "diff", "--module", "Kitchen"]);
    let rendered = format!(
        "exit: {code:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        normalize(&stdout, &target),
        normalize(&stderr, &target),
    );
    insta::assert_snapshot!(rendered);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn items_diff_unknown_kind_error_is_stable() {
    let target = build_project("unknown-kind");
    std::fs::write(
        target.join("specs/modules/Kitchen.toml"),
        r#"name = "Kitchen"
visibility = "pub"

[[items]]
name = "oops"
kind = "gadget"
description = "Unknown kind must fail with the vocabulary listed."
satisfies = ["DEMO-1"]
"#,
    )
    .unwrap();
    let (code, stdout, stderr) = run_cli(&target, &["items", "diff", "--module", "Kitchen"]);
    let rendered = format!(
        "exit: {code:?}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        normalize(&stdout, &target),
        normalize(&stderr, &target),
    );
    insta::assert_snapshot!(rendered);
    let _ = std::fs::remove_dir_all(&target);
}
