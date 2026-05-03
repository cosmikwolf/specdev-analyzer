//! Tree-sitter item extraction.
//!
//! Phase 10 (per D40, 2026-05-02) ships `extract-items` as the surface
//! that Phase 14's items[] sync builds on. The output shape
//! (`ExtractedItem`) is the strict subset of the spec-side `Item`
//! schema: `kind` + `name` + optional `signature` + optional
//! `attributes[]` + source span. Spec-only fields (`description`,
//! `satisfies`) are out of scope for the analyzer — those are spec
//! authoring concerns.
//!
//! The extraction pipeline:
//! 1. Resolve module name → source file path (via [`source_map`]).
//! 2. Read the source file (no macro expansion in v1).
//! 3. Parse with tree-sitter using the language adapter
//!    ([`crate::lang::LanguageAdapter`]).
//! 4. Run the language's `items.scm` query and map captures to
//!    `ExtractedItem` per capture name (`<kind>.def` + `<kind>.name`).
//! 5. Emit YAML.
//!
//! The 12 Rust kinds (`fn` / `struct` / `enum` / `union` / `trait` /
//! `impl` / `type` / `const` / `static` / `re_export` / `macro` /
//! `extern_block`) are 1:1 with the items.schema.yaml kind enum and
//! tree-sitter Rust grammar nodes. See
//! `specdev/languages/rust/items.schema.yaml` and
//! `specdev/languages/rust/items.scm` for authoritative definitions.

pub mod extract;
pub mod source_map;
pub mod util;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractedItem {
    /// Item kind — matches the items.schema.yaml kind enum.
    pub kind: ItemKind,
    /// Item name (identifier as written in source). For `impl` items,
    /// synthesized as `<TraitRef> for <TargetType>` (trait impl) or
    /// `<TargetType>` (inherent impl).
    pub name: String,
    /// Optional signature shape, populated when the items.scm query
    /// surfaces it (functions: `fn foo(...) -> Bar`; consts:
    /// `pub const FOO: u32`). Reserved for future use; v1 may emit
    /// `None` for most kinds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
    /// Attribute strings as written (e.g. `#[test]`, `#[embassy_executor::task]`,
    /// `#[cfg(feature = "alloc")]`). Empty when no attributes precede
    /// the item.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
    /// Source span (1-based line numbers) where the item is defined.
    pub span: Span,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Fn,
    Struct,
    Enum,
    Union,
    Trait,
    Impl,
    Type,
    Const,
    Static,
    ReExport,
    Macro,
    ExternBlock,
}

impl ItemKind {
    /// Schema-facing string form (matches items.schema.yaml enum).
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Fn => "fn",
            ItemKind::Struct => "struct",
            ItemKind::Enum => "enum",
            ItemKind::Union => "union",
            ItemKind::Trait => "trait",
            ItemKind::Impl => "impl",
            ItemKind::Type => "type",
            ItemKind::Const => "const",
            ItemKind::Static => "static",
            ItemKind::ReExport => "re_export",
            ItemKind::Macro => "macro",
            ItemKind::ExternBlock => "extern_block",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Span {
    /// First line of the item (1-based, inclusive).
    pub start_line: usize,
    /// Last line of the item (1-based, inclusive).
    pub end_line: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleItems {
    pub module_name: String,
    pub source_file: Option<std::path::PathBuf>,
    pub items: Vec<ExtractedItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionReport {
    pub modules: Vec<ModuleItems>,
}
