//! Tree-sitter item extraction.
//!
//! Ships `extract-items` as the surface that items[] sync builds on.
//! The output shape
//! (`ExtractedItem`) is the strict subset of the spec-side `Item`
//! schema: `kind` + `name` + optional `attributes[]` + source span
//! (an item carries no signature). Spec-only fields (`description`,
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
//! The item-kind vocabulary is per-language DATA, not analyzer code: the
//! authoritative list is the `kind` enum in the project's inlined
//! `languages/<lang>/items.schema.yaml` (loaded at runtime by
//! [`load_item_kinds`]), and the language's `items.scm` capture prefixes
//! carry the same strings — the extractor derives each item's `kind`
//! from its capture prefix. Adding a language (or a kind) is a
//! language-tier content change; no enum here to widen.

pub mod apply;
pub mod extract;
pub mod propose;
pub mod source_map;
pub mod sync;
pub mod util;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::layout::LayoutContext;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractedItem {
    /// Item kind — a string from the language tier's items.schema.yaml
    /// kind enum (equal to the items.scm capture prefix that produced
    /// this item).
    pub kind: String,
    /// Item name (identifier as written in source). For `impl` items,
    /// synthesized as `<TraitRef> for <TargetType>` (trait impl). For
    /// members of a type (inherent impl members, Dart class members),
    /// qualified as `<Type><sep><member>` with the language's path
    /// separator.
    pub name: String,
    /// Attribute strings as written (e.g. `#[test]`, `#[embassy_executor::task]`,
    /// `#[cfg(feature = "alloc")]`). Empty when no attributes precede
    /// the item.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
    /// Source span (1-based line numbers) where the item is defined.
    pub span: Span,
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

/// Load the project language's item-kind vocabulary from the inlined
/// language tier (`specs/specdev/schemas/inlined/languages/<lang>/
/// items.schema.yaml` → `properties.kind.enum`). This is the same file
/// that schema-validates spec `[[items]]` via the profile `$ref`, so the
/// two gates cannot drift.
///
/// Errors when the tier file is missing or carries no kind enum —
/// callers surface this only for modules that actually declare items
/// (languages without an items vocabulary still bootstrap and validate;
/// they just have no items substrate).
pub fn load_item_kinds(layout: &LayoutContext) -> Result<Vec<String>> {
    let lang = &layout.spec.project.language;
    let path = layout
        .inlined_languages_dir
        .join(lang)
        .join("items.schema.yaml");
    let text = std::fs::read_to_string(&path).map_err(|_| {
        anyhow!(
            "item-kind vocabulary not found at {} — the `{lang}` language tier \
             (languages/{lang}/items.schema.yaml) is not inlined in this project; \
             re-bootstrap or run `specdev migrate`",
            path.display()
        )
    })?;
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let kinds: Vec<String> = doc
        .get("properties")
        .and_then(|p| p.get("kind"))
        .and_then(|k| k.get("enum"))
        .and_then(|e| e.as_sequence())
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if kinds.is_empty() {
        return Err(anyhow!(
            "{} declares no `properties.kind.enum` — the language tier's kind \
             vocabulary is empty or malformed",
            path.display()
        ));
    }
    Ok(kinds)
}
