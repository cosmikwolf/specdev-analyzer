//! Spec module name → on-disk source file resolution.
//!
//! Phase 10 convention (per the items[] sequencing in
//! `05-implementation-plan.md`):
//!
//! 1. Convention: `<project>/<source_root>/<snake_case(name)>.<ext>`.
//! 2. For Rust + flat layout, also try `<snake_case(name)>/mod.<ext>`.
//!
//! Phase 14 may add `module.metadata.SourceMapping.file` overrides as
//! items[] sync surfaces real needs. For now we only honour the
//! convention because pre-Phase-14 specs don't carry items[] yet, and
//! `verified_by_resolves` (Phase 10's first consumer) only needs
//! convention-based resolution.

use std::path::{Path, PathBuf};

use crate::lang::LanguageAdapter;

use super::util::snake_case;

/// Resolve a spec module name to a source file path. Returns `None`
/// when no convention-matching file exists — pre-implementation
/// modules return `None` and the caller should treat that as "no
/// items extracted," not as an error.
pub fn resolve_source_path(
    module_name: &str,
    adapter: &dyn LanguageAdapter,
    project_root: &Path,
    source_root: &str,
) -> Option<PathBuf> {
    let ext = adapter.file_extension();
    let snake = snake_case(module_name);

    let flat = project_root
        .join(source_root)
        .join(format!("{snake}.{ext}"));
    if flat.exists() {
        return Some(flat);
    }
    if adapter.name() == "rust" {
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
