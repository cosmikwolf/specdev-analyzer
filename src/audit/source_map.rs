//! Source file resolution: spec module → on-disk source file.
//!
//! Priority:
//!   1. `module.metadata.SourceMapping.file` — explicit override relative
//!      to the project root.
//!   2. Convention: `<project>/<source_root>/<snake_case(name)>.<ext>`.
//!   3. For "flat" layout, also tries `<snake_case(name)>/mod.<ext>` (Rust).

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::lang::LanguageAdapter;

use super::util::snake_case;

pub fn resolve_source_path(
    module: &Value,
    adapter: &dyn LanguageAdapter,
    project_root: &Path,
    source_root: &str,
    layout: &str,
) -> Option<PathBuf> {
    if let Some(s) = module
        .get("metadata")
        .and_then(|m| m.get("SourceMapping"))
        .and_then(|m| m.get("file"))
        .and_then(Value::as_str)
    {
        let path = project_root.join(s);
        if path.exists() {
            return Some(path);
        }
    }

    let name = module.get("name").and_then(Value::as_str)?;
    let ext = adapter.file_extension();
    let snake = snake_case(name);

    let flat = project_root
        .join(source_root)
        .join(format!("{snake}.{ext}"));
    if flat.exists() {
        return Some(flat);
    }
    if layout == "flat" && adapter.name() == "rust" {
        let dir = project_root
            .join(source_root)
            .join(&snake)
            .join(format!("mod.{ext}"));
        if dir.exists() {
            return Some(dir);
        }
    }
    None
}
