//! Dart adapter — tree-sitter-dart + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct DartAdapter;

const QUERY: &str = include_str!("../embedded/languages/dart/audit.scm");
const ITEMS_QUERY: &str = include_str!("../embedded/languages/dart/items.scm");
const IMPORTS_QUERY: &str = include_str!("../embedded/languages/dart/imports.scm");

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

    fn items_query(&self) -> Option<&'static str> {
        Some(ITEMS_QUERY)
    }

    fn imports_query(&self) -> Option<&'static str> {
        Some(IMPORTS_QUERY)
    }

    /// `'package:name/x.dart'` → `name`, unless `name` is the project's
    /// own package. `dart:` URIs and relative URIs name no package.
    fn import_package(&self, specifier: &str, self_package: &str) -> Option<String> {
        let s = specifier.trim().trim_matches(|c| c == '\'' || c == '"');
        let rest = s.strip_prefix("package:")?;
        let name = rest.split('/').next().filter(|n| !n.is_empty())?;
        (name != self_package).then(|| name.to_string())
    }

    fn lockfile_candidates(&self) -> &'static [&'static str] {
        &["pubspec.lock"]
    }

    fn item_is_public_name(&self, name: &str) -> bool {
        // Dart privacy is lexical: a `_`-prefixed identifier is
        // library-private. Qualified names (`Device.connect`) are private
        // when ANY segment is — a public member of a private class is
        // not reachable API.
        name.split('.').all(|seg| !seg.starts_with('_'))
    }

    fn annotation_suppresses_item(&self, annotation: &str) -> bool {
        // `@override` members implement inherited contract (a widget's
        // `build`, `createState`, `toString`); the contract lives on the
        // declaring base, so overrides are not new public surface.
        annotation == "override"
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
