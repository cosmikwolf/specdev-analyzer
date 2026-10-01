//! C adapter — tree-sitter-c + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct CAdapter;

const QUERY: &str = include_str!("../embedded/languages/c/audit.scm");
const TESTS_QUERY: &str = include_str!("../embedded/languages/c/tests.scm");

impl LanguageAdapter for CAdapter {
    fn tests_query(&self) -> Option<&'static str> {
        Some(TESTS_QUERY)
    }

    fn name(&self) -> &'static str {
        "c"
    }

    fn file_extension(&self) -> &'static str {
        "c"
    }

    fn tree_sitter_language(&self) -> Language {
        tree_sitter_c::LANGUAGE.into()
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
                "identifier" | "pointer_declarator" => {
                    name = source[child.byte_range()].to_string();
                }
                "type_identifier" | "primitive_type" | "sized_type_specifier" => {
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
