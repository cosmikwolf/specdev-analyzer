//! C++ adapter — tree-sitter-cpp + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct CppAdapter;

const QUERY: &str = include_str!("../embedded/languages/cpp/audit.scm");

impl LanguageAdapter for CppAdapter {
    fn name(&self) -> &'static str {
        "cpp"
    }

    fn file_extension(&self) -> &'static str {
        "cpp"
    }

    fn tree_sitter_language(&self) -> Language {
        tree_sitter_cpp::LANGUAGE.into()
    }

    fn embedded_query(&self) -> &'static str {
        QUERY
    }

    fn parameter_node_kind(&self) -> &'static str {
        "parameter_declaration"
    }

    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        let mut name = String::new();
        let mut type_name = String::new();
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" | "pointer_declarator" | "reference_declarator" => {
                    name = source[child.byte_range()].to_string();
                }
                "type_identifier"
                | "primitive_type"
                | "sized_type_specifier"
                | "qualified_identifier"
                | "template_type" => {
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
}
