//! Dart adapter — tree-sitter-dart + embedded `audit.scm`.

use std::path::Path;
use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct DartAdapter;

const QUERY: &str = include_str!("../embedded/languages/dart/audit.scm");
const ITEMS_QUERY: &str = include_str!("../embedded/languages/dart/items.scm");
const IMPORTS_QUERY: &str = include_str!("../embedded/languages/dart/imports.scm");
const TESTS_QUERY: &str = include_str!("../embedded/languages/dart/tests.scm");

impl LanguageAdapter for DartAdapter {
    fn manifest_facts(
        &self,
        project_root: &std::path::Path,
    ) -> Option<crate::manifest_facts::ManifestFacts> {
        crate::manifest_facts::pub_(project_root)
    }

    fn is_inert_top_level(&self, kind: &str) -> bool {
        kind.contains("comment")
            || [
                "import_or_export",
                "library_name",
                "part_directive",
                "part_of_directive",
            ]
            .contains(&kind)
    }

    fn tests_query(&self) -> Option<&'static str> {
        Some(TESTS_QUERY)
    }

    fn test_identity_style(&self) -> super::TestIdentityStyle {
        super::TestIdentityStyle::GroupPath(" > ")
    }

    fn conventional_test_roots(&self) -> &'static [&'static str] {
        &["test", "integration_test"]
    }

    /// `*_test.dart`, the package:test convention.
    fn is_test_file(&self, rel: &Path) -> bool {
        rel.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_test.dart"))
    }

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
    /// `package:<own package>/x.dart` → `lib/x.dart`; a relative URI from the
    /// importing file's directory; `dart:` and other packages are external.
    fn resolve_import(
        &self,
        spec: &str,
        from: &Path,
        ctx: &super::ImportContext,
    ) -> Vec<std::path::PathBuf> {
        let s = spec.trim().trim_matches(|c| c == '\'' || c == '"');
        if let Some(rest) = s.strip_prefix("package:") {
            let (pkg, path) = rest.split_once('/').unwrap_or((rest, ""));
            if pkg == ctx.package_name && !path.is_empty() {
                return vec![Path::new("lib").join(path)];
            }
            return Vec::new();
        }
        if s.contains(':') {
            return Vec::new();
        }
        let dir = from.parent().unwrap_or(Path::new(""));
        vec![super::normalize_rel(&dir.join(s))]
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
