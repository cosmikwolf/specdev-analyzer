//! Language adapters for the audit pipeline (Phase 8).
//!
//! Each adapter binds a spec `language` string to a tree-sitter grammar,
//! an embedded `.scm` query, and a per-language parameter parser. Per D9
//! the rest of the analyzer must never switch on language by name — it
//! resolves an adapter through [`adapter_for`] and calls trait methods.

pub mod c;
pub mod cpp;
pub mod rust;

use tree_sitter::{Language, Node};

/// A parsed function parameter, as observed in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedParameter {
    pub name: String,
    pub type_name: String,
}

/// A tree-sitter-backed language adapter.
pub trait LanguageAdapter: Send + Sync {
    /// Canonical name — must match `spec.toml`'s `project.language`.
    fn name(&self) -> &'static str;

    /// Source file extension used when resolving modules to files.
    fn file_extension(&self) -> &'static str;

    /// Tree-sitter grammar for this language.
    fn tree_sitter_language(&self) -> Language;

    /// The built-in `.scm` query source. Adapters embed this via `include_str!`
    /// so the analyzer binary is self-contained; plugins can still override
    /// by supplying their own query via `queries_dir`.
    fn embedded_query(&self) -> &'static str;

    /// Parse a single parameter node from this language's grammar, if the
    /// adapter recognizes its kind.
    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter>;

    /// Some languages (Rust) expose explicit `self` parameters as a distinct
    /// node kind. Override to lift them into the parameter list.
    fn parse_self_parameter(&self, _node: Node<'_>, _source: &str) -> Option<ParsedParameter> {
        None
    }

    /// The grammar's node kind for items inside a parameter list that
    /// correspond to ordinary parameters. `parse_parameter` is only called
    /// on children whose kind matches this or [`self_node_kind`].
    fn parameter_node_kind(&self) -> &'static str;

    /// Optional — the node kind for a self/receiver parameter, if any.
    fn self_node_kind(&self) -> Option<&'static str> {
        None
    }
}

/// Resolve a language name to a boxed adapter. Returns `None` for unknown
/// languages; callers surface this with their own error context.
pub fn adapter_for(language: &str) -> Option<Box<dyn LanguageAdapter>> {
    match language {
        "rust" => Some(Box::new(rust::RustAdapter)),
        "c" => Some(Box::new(c::CAdapter)),
        "cpp" | "c++" => Some(Box::new(cpp::CppAdapter)),
        _ => None,
    }
}
