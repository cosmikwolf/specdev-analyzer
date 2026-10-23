//! Spec module name → on-disk source file resolution.
//!
//! Precedence (#101(a)):
//!
//! 1. The module spec's explicit `source_file` override (project-root-
//!    relative), when the file exists.
//! 2. The language adapter's ordered candidate list under
//!    `<project>/<source_root>/` — `<snake>.<ext>` for every language,
//!    plus whatever the adapter adds (Rust: `<snake>/mod.rs`), for each
//!    stem in [`snake_case_variants`] in order. The first candidate that
//!    exists wins.
//! 3. Nothing — a pre-implementation module. `None`, never an error.
//!
//! No branch here reads the language *name*: a layout convention is the
//! adapter's to declare ([`LanguageAdapter::module_file_candidates`]).

use std::path::{Path, PathBuf};

use crate::lang::LanguageAdapter;

use super::util::snake_case_variants;

/// Resolve a spec module name to a source file path. `source_file` is the
/// module spec's explicit override, if declared, relative to the code
/// root, as `source_root` is. Returns `None` when
/// neither the override nor any convention candidate exists on disk —
/// callers treat that as "no items extracted," not as an error.
pub fn resolve_source_path(
    module_name: &str,
    source_file: Option<&str>,
    adapter: &dyn LanguageAdapter,
    code_root: &Path,
    source_root: &str,
) -> Option<PathBuf> {
    if let Some(rel) = source_file.map(str::trim).filter(|s| !s.is_empty()) {
        let explicit = code_root.join(rel);
        if explicit.is_file() {
            return Some(explicit);
        }
        // A declared file that is not on disk resolves like a
        // pre-implementation module (schema prose says so); the
        // convention candidates are NOT tried — the author named a file.
        return None;
    }
    let base = code_root.join(source_root);
    snake_case_variants(module_name)
        .iter()
        .flat_map(|snake| adapter.module_file_candidates(snake))
        .map(|rel| base.join(rel))
        .find(|p| p.is_file())
}

/// The `source_file` override declared on a parsed module spec, if any.
pub fn source_file_override(module_toml: &toml::Value) -> Option<&str> {
    module_toml.get("source_file").and_then(|v| v.as_str())
}

/// Read a module spec from disk and return its `source_file` override.
/// Any read or parse failure yields `None` — the spec's own validity is
/// the validate pass's business, not the resolver's.
pub fn load_source_file_override(module_path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(module_path).ok()?;
    let value: toml::Value = toml::from_str(&text).ok()?;
    source_file_override(&value).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::rust::RustAdapter;

    fn tmp(label: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("specdev-source-map-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src/tasks")).unwrap();
        d
    }

    #[test]
    fn digit_names_resolve_both_ways() {
        // #253: `Aced2Format` lives at `aced2_format.rs` (heck's split),
        // `SysEx7` at `sysex7.rs` (no break at all).
        let root = tmp("digits");
        std::fs::write(root.join("src/aced2_format.rs"), "").unwrap();
        std::fs::write(root.join("src/sysex7.rs"), "").unwrap();
        let a = RustAdapter;
        assert_eq!(
            resolve_source_path("Aced2Format", None, &a, &root, "src"),
            Some(root.join("src/aced2_format.rs"))
        );
        assert_eq!(
            resolve_source_path("SysEx7", None, &a, &root, "src"),
            Some(root.join("src/sysex7.rs"))
        );
    }

    #[test]
    fn flat_then_mod_rs_by_adapter_candidates() {
        let root = tmp("candidates");
        std::fs::write(root.join("src/foo.rs"), "").unwrap();
        std::fs::create_dir_all(root.join("src/bar")).unwrap();
        std::fs::write(root.join("src/bar/mod.rs"), "").unwrap();
        let a = RustAdapter;
        assert_eq!(
            resolve_source_path("Foo", None, &a, &root, "src"),
            Some(root.join("src/foo.rs"))
        );
        assert_eq!(
            resolve_source_path("Bar", None, &a, &root, "src"),
            Some(root.join("src/bar/mod.rs"))
        );
        assert_eq!(resolve_source_path("Baz", None, &a, &root, "src"), None);
    }

    // #101(a): a nested task-body module or a single-file crate root is
    // reachable only through the explicit override.
    #[test]
    fn explicit_override_wins_and_a_missing_override_is_pre_implementation() {
        let root = tmp("override");
        std::fs::write(root.join("src/tasks/blink.rs"), "").unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        // The convention file also exists — the override still wins.
        std::fs::write(root.join("src/blink.rs"), "").unwrap();
        let a = RustAdapter;
        assert_eq!(
            resolve_source_path("Blink", Some("src/tasks/blink.rs"), &a, &root, "src"),
            Some(root.join("src/tasks/blink.rs"))
        );
        assert_eq!(
            resolve_source_path("Lib", Some("src/lib.rs"), &a, &root, "src"),
            Some(root.join("src/lib.rs"))
        );
        assert_eq!(
            resolve_source_path("Blink", Some("src/nope.rs"), &a, &root, "src"),
            None
        );
        // Blank override = no override.
        assert_eq!(
            resolve_source_path("Blink", Some("  "), &a, &root, "src"),
            Some(root.join("src/blink.rs"))
        );
    }

    #[test]
    fn override_is_read_from_the_module_toml() {
        let root = tmp("toml");
        let m = root.join("Blink.toml");
        std::fs::write(
            &m,
            "name = \"Blink\"\nsource_file = \"src/tasks/blink.rs\"\n",
        )
        .unwrap();
        assert_eq!(
            load_source_file_override(&m).as_deref(),
            Some("src/tasks/blink.rs")
        );
        std::fs::write(&m, "name = \"Blink\"\n").unwrap();
        assert_eq!(load_source_file_override(&m), None);
        assert_eq!(load_source_file_override(&root.join("missing.toml")), None);
    }
}
