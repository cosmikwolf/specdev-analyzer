//! Language adapters for the audit / reverse-engineering pipeline.
//!
//! Per D9, any tree-sitter-compatible language can be added by implementing
//! `LanguageAdapter`, providing `.scm` query files, and registering the adapter
//! with the dispatcher. The core analyzer must never reference a concrete
//! language by name.

pub mod c;
pub mod cpp;
pub mod rust;

/// The canonical data shape extracted adapters map source code into.
///
/// Phase 8 will expand this. Left empty now so every adapter compiles.
#[derive(Debug, Default)]
pub struct ModuleLike {}

/// A tree-sitter-backed language adapter.
pub trait LanguageAdapter {
    /// Canonical name (must match `spec.toml`'s `language` field).
    fn name(&self) -> &'static str;

    /// Parse a source file and emit the canonical shape consumed by audit.
    fn extract(&self, _source: &str) -> anyhow::Result<Vec<ModuleLike>> {
        // Default: unimplemented until Phase 8.
        Ok(Vec::new())
    }
}
