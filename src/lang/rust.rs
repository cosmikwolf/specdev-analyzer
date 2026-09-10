//! Rust adapter — tree-sitter-rust + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct RustAdapter;

const QUERY: &str = include_str!("../../../specdev/languages/rust/audit.scm");
const ITEMS_QUERY: &str = include_str!("../../../specdev/languages/rust/items.scm");

impl LanguageAdapter for RustAdapter {
    fn name(&self) -> &'static str {
        "rust"
    }

    fn file_extension(&self) -> &'static str {
        "rs"
    }

    fn tree_sitter_language(&self) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn embedded_query(&self) -> &'static str {
        QUERY
    }

    fn items_query(&self) -> Option<&'static str> {
        Some(ITEMS_QUERY)
    }

    fn module_file_candidates(&self, snake: &str) -> Vec<String> {
        vec![format!("{snake}.rs"), format!("{snake}/mod.rs")]
    }

    fn lockfile_candidates(&self) -> &'static [&'static str] {
        &["Cargo.lock"]
    }

    fn toolchain_file_candidates(&self) -> &'static [&'static str] {
        &["rust-toolchain.toml", "rust-toolchain"]
    }

    fn default_visibility_floor(&self) -> Option<&'static str> {
        Some("pub")
    }

    fn visibility_at_least(&self, captured: Option<&str>, floor: &str) -> bool {
        visibility_rank(captured) >= visibility_rank(Some(floor))
    }

    fn re_export_visibility(&self, captured: &str) -> Option<&'static str> {
        match normalize_visibility(captured).as_str() {
            "pub" => Some("pub"),
            "pub(crate)" => Some("crate"),
            _ => None,
        }
    }

    fn item_identity_key(&self, kind: &str, name: &str) -> String {
        // Trait-impl names carry generics and path qualifiers the author
        // shouldn't have to track; normalize them out of the join key.
        if kind == "impl" {
            crate::items::sync::normalize_impl_name(name)
        } else {
            name.to_string()
        }
    }

    fn is_test_function(&self, item: &crate::items::ExtractedItem) -> bool {
        item.kind == "fn" && item.attributes.iter().any(|a| a.contains("test"))
    }

    fn parameter_node_kind(&self) -> &'static str {
        "parameter"
    }

    fn self_node_kind(&self) -> Option<&'static str> {
        Some("self_parameter")
    }

    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        let mut name = String::new();
        let mut type_name = String::new();
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" if name.is_empty() => {
                    name = source[child.byte_range()].to_string();
                }
                "reference_type"
                | "type_identifier"
                | "generic_type"
                | "scoped_type_identifier"
                | "primitive_type"
                | "array_type"
                | "tuple_type" => {
                    type_name = source[child.byte_range()].to_string();
                }
                _ => {}
            }
        }

        if name.is_empty() {
            None
        } else {
            Some(ParsedParameter { name, type_name })
        }
    }

    fn parse_self_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        let text = source[node.byte_range()].to_string();
        Some(ParsedParameter {
            name: "self".to_string(),
            type_name: text,
        })
    }
}

/// Collapse whitespace inside a visibility modifier so `pub (crate)` and
/// `pub(crate)` compare equal.
fn normalize_visibility(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Rust's visibility lattice as a rank. `pub` is the widest; `pub(self)`
/// and no modifier are private. `pub(in path)` is treated like
/// `pub(super)`: narrower than the crate, wider than the module.
fn visibility_rank(vis: Option<&str>) -> u8 {
    match vis.map(normalize_visibility).as_deref() {
        Some("pub") => 3,
        Some("pub(crate)") => 2,
        Some("pub(super)") => 1,
        Some(v) if v.starts_with("pub(in") => 1,
        _ => 0,
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    use crate::lang::LanguageAdapter;

    #[test]
    fn pub_floor_admits_only_pub() {
        let a = RustAdapter;
        assert!(a.visibility_at_least(Some("pub"), "pub"));
        assert!(!a.visibility_at_least(Some("pub(crate)"), "pub"));
        assert!(!a.visibility_at_least(Some("pub(super)"), "pub"));
        assert!(!a.visibility_at_least(None, "pub"));
    }

    #[test]
    fn crate_floor_admits_pub_and_pub_crate() {
        let a = RustAdapter;
        assert!(a.visibility_at_least(Some("pub"), "pub(crate)"));
        assert!(a.visibility_at_least(Some("pub (crate)"), "pub(crate)"));
        assert!(!a.visibility_at_least(Some("pub(super)"), "pub(crate)"));
        assert!(!a.visibility_at_least(Some("pub(in crate::x)"), "pub(crate)"));
        assert!(!a.visibility_at_least(None, "pub(crate)"));
    }

    #[test]
    fn re_export_visibility_follows_the_spec_enum() {
        let a = RustAdapter;
        assert_eq!(a.re_export_visibility("pub"), Some("pub"));
        assert_eq!(a.re_export_visibility("pub(crate)"), Some("crate"));
        assert_eq!(a.re_export_visibility("pub(super)"), None);
        assert_eq!(a.re_export_visibility("pub(in crate::a)"), None);
    }
}
