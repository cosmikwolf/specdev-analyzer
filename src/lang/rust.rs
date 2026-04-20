//! Rust adapter — tree-sitter-rust + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct RustAdapter;

const QUERY: &str = include_str!("../../../specdev/languages/rust/audit.scm");

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
