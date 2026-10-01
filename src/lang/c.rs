//! C adapter — tree-sitter-c + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct CAdapter;

const QUERY: &str = include_str!("../embedded/languages/c/audit.scm");
const TESTS_QUERY: &str = include_str!("../embedded/languages/c/tests.scm");
const IMPORTS_QUERY: &str = include_str!("../embedded/languages/c/imports.scm");

impl LanguageAdapter for CAdapter {
    fn tests_query(&self) -> Option<&'static str> {
        Some(TESTS_QUERY)
    }

    fn imports_query(&self) -> Option<&'static str> {
        Some(IMPORTS_QUERY)
    }

    /// A quoted include, from the including file's directory, then from the
    /// source root.
    fn resolve_import(&self, spec: &str, from: &std::path::Path, ctx: &super::ImportContext) -> Vec<std::path::PathBuf> {
        let dir = from.parent().unwrap_or(std::path::Path::new(""));
        vec![
            super::normalize_rel(&dir.join(spec)),
            super::normalize_rel(&ctx.source_root.join(spec)),
        ]
    }

    /// The header beside a source file belongs to its module.
    fn module_twin_files(&self, source: &std::path::Path) -> Vec<std::path::PathBuf> {
        ["h"].iter().map(|ext| source.with_extension(ext)).collect()
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
