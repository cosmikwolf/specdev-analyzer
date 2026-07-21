//! Dart adapter — tree-sitter-dart + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct DartAdapter;

const QUERY: &str = include_str!("../../../specdev/languages/dart/audit.scm");

impl LanguageAdapter for DartAdapter {
    fn name(&self) -> &'static str {
        "dart"
    }

    fn file_extension(&self) -> &'static str {
        "dart"
    }

    fn path_separator(&self) -> &'static str {
        "."
    }

    fn tree_sitter_language(&self) -> Language {
        tree_sitter_dart::LANGUAGE.into()
    }

    fn embedded_query(&self) -> &'static str {
        QUERY
    }

    fn parameter_node_kind(&self) -> &'static str {
        "formal_parameter"
    }

    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        // `formal_parameter` puts the name behind a field; the type (when
        // present — Dart allows untyped params) is a plain `type` child.
        let name = node
            .child_by_field_name("name")
            .map(|n| source[n.byte_range()].to_string())?;
        let mut type_name = String::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "type" {
                type_name = source[child.byte_range()].to_string();
            }
        }
        Some(ParsedParameter { name, type_name })
    }
}
