//! Python adapter — tree-sitter-python + the embedded queries.
//!
//! Privacy is lexical (a `_` prefix), as in Dart; paths use `.`; tests follow
//! pytest's discovery rules and are named by the pytest node id when a bare
//! name is ambiguous. The adapter names no tool: the manifest is
//! `pyproject.toml` `[project]` (PEP 621), and every common lockfile is listed.

use std::path::{Path, PathBuf};
use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct PythonAdapter;

const QUERY: &str = include_str!("../embedded/languages/python/audit.scm");
const ITEMS_QUERY: &str = include_str!("../embedded/languages/python/items.scm");
const IMPORTS_QUERY: &str = include_str!("../embedded/languages/python/imports.scm");
const TESTS_QUERY: &str = include_str!("../embedded/languages/python/tests.scm");

/// `a.b.c` → `a/b/c.py`, `a/b/c.pyi`, `a/b/c/__init__.py` under `base`.
fn module_files(base: &Path, dotted: &str) -> Vec<PathBuf> {
    let rel: PathBuf = dotted.split('.').filter(|s| !s.is_empty()).collect();
    if rel.as_os_str().is_empty() {
        return vec![base.join("__init__.py")];
    }
    vec![
        base.join(&rel).with_extension("py"),
        base.join(&rel).with_extension("pyi"),
        base.join(&rel).join("__init__.py"),
    ]
}

impl LanguageAdapter for PythonAdapter {
    fn name(&self) -> &'static str {
        "python"
    }

    fn file_extension(&self) -> &'static str {
        "py"
    }

    fn file_extensions(&self) -> Vec<&'static str> {
        vec!["py", "pyi"]
    }

    fn path_separator(&self) -> &'static str {
        "."
    }

    fn tree_sitter_language(&self) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn embedded_query(&self) -> &'static str {
        QUERY
    }

    fn items_query(&self) -> Option<&'static str> {
        Some(ITEMS_QUERY)
    }

    fn tests_query(&self) -> Option<&'static str> {
        Some(TESTS_QUERY)
    }

    fn imports_query(&self) -> Option<&'static str> {
        Some(IMPORTS_QUERY)
    }

    fn test_identity_style(&self) -> super::TestIdentityStyle {
        super::TestIdentityStyle::FileGroupPath("::")
    }

    fn conventional_test_roots(&self) -> &'static [&'static str] {
        &["tests"]
    }

    /// pytest's default file patterns: `test_*.py` and `*_test.py`.
    fn is_test_file(&self, rel: &Path) -> bool {
        rel.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
            (n.starts_with("test_") && n.ends_with(".py")) || n.ends_with("_test.py")
        })
    }

    fn is_inert_top_level(&self, kind: &str) -> bool {
        kind.contains("comment")
            || [
                "import_statement",
                "import_from_statement",
                "future_import_statement",
            ]
            .contains(&kind)
    }

    /// `foo_bar.py`, then the package `foo_bar/__init__.py`; a stub
    /// `foo_bar.pyi` last (the native half of an extension module).
    fn module_file_candidates(&self, snake: &str) -> Vec<String> {
        vec![
            format!("{snake}.py"),
            format!("{snake}/__init__.py"),
            format!("{snake}.pyi"),
        ]
    }

    /// `from . import stream` → `.stream`; `from pkg import sub` → `pkg.sub`.
    fn member_import_spec(&self, module: &str, member: &str) -> Option<String> {
        Some(if module.ends_with('.') {
            format!("{module}{member}")
        } else {
            format!("{module}.{member}")
        })
    }

    /// A relative module (`.a`, `..a.b`, `.`) from the importing file's
    /// package; an absolute one (`pkg.a`) under the source root — whose last
    /// segment is the top package in a `src/<pkg>` layout — or below it.
    /// Candidates that are not project files are dropped by the caller, so a
    /// third-party module resolves to nothing (external).
    fn resolve_import(&self, spec: &str, from: &Path, ctx: &super::ImportContext) -> Vec<PathBuf> {
        let spec = spec.trim();
        let dots = spec.chars().take_while(|c| *c == '.').count();
        if dots > 0 {
            let mut base = from.parent().unwrap_or(Path::new("")).to_path_buf();
            for _ in 1..dots {
                base = base.parent().unwrap_or(Path::new("")).to_path_buf();
            }
            return module_files(&base, &spec[dots..])
                .into_iter()
                .map(|p| super::normalize_rel(&p))
                .collect();
        }
        let root = &ctx.source_root;
        let mut out = Vec::new();
        let first = spec.split('.').next().unwrap_or("");
        if root.file_name().and_then(|n| n.to_str()) == Some(first) {
            let parent = root.parent().unwrap_or(Path::new(""));
            out.extend(module_files(parent, spec));
        }
        out.extend(module_files(root, spec));
        out.into_iter().map(|p| super::normalize_rel(&p)).collect()
    }

    fn manifest_facts(&self, project_root: &Path) -> Option<crate::manifest_facts::ManifestFacts> {
        crate::manifest_facts::pyproject(project_root)
    }

    fn lockfile_candidates(&self) -> &'static [&'static str] {
        &["uv.lock", "poetry.lock", "pdm.lock", "pylock.toml"]
    }

    fn toolchain_file_candidates(&self) -> &'static [&'static str] {
        &[".python-version"]
    }

    /// Privacy is lexical: a `_`-prefixed name is private, and so is every
    /// member of a private class (`_Hidden.visible`). Dunder methods start
    /// with `_` and drop with the rest.
    fn item_is_public_name(&self, name: &str) -> bool {
        name.split('.').all(|seg| !seg.starts_with('_'))
    }

    fn parameter_node_kind(&self) -> &'static str {
        "typed_parameter"
    }

    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        // `typed_parameter`: an identifier child, then `type: (type)`.
        let mut cursor = node.walk();
        let name = node
            .children(&mut cursor)
            .find(|c| c.kind() == "identifier")
            .map(|n| source[n.byte_range()].to_string())?;
        let type_name = node
            .child_by_field_name("type")
            .map(|n| source[n.byte_range()].to_string())
            .unwrap_or_default();
        Some(ParsedParameter { name, type_name })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::ImportContext;

    fn ctx(source_root: &str) -> ImportContext {
        ImportContext {
            code_root: PathBuf::from("/p"),
            source_root: PathBuf::from(source_root),
            package_name: "my-package".into(),
            aliases: Vec::new(),
        }
    }

    #[test]
    fn relative_imports_resolve_from_the_package() {
        let a = PythonAdapter;
        let from = Path::new("src/pkg/sub/mod.py");
        let c = ctx("src/pkg");
        assert!(a
            .resolve_import(".sibling", from, &c)
            .contains(&PathBuf::from("src/pkg/sub/sibling.py")));
        assert!(a
            .resolve_import("..other", from, &c)
            .contains(&PathBuf::from("src/pkg/other.py")));
        assert!(a
            .resolve_import(".", from, &c)
            .contains(&PathBuf::from("src/pkg/sub/__init__.py")));
    }

    #[test]
    fn absolute_imports_resolve_under_the_source_root() {
        let a = PythonAdapter;
        let from = Path::new("tests/test_x.py");
        let got = a.resolve_import("pkg.util", from, &ctx("src/pkg"));
        assert!(got.contains(&PathBuf::from("src/pkg/util.py")), "{got:?}");
        let got = a.resolve_import("pkg.util", from, &ctx("src"));
        assert!(got.contains(&PathBuf::from("src/pkg/util.py")), "{got:?}");
    }

    #[test]
    fn privacy_is_by_name() {
        let a = PythonAdapter;
        assert!(a.item_is_public_name("Packet.parse"));
        assert!(!a.item_is_public_name("Packet.__init__"));
        assert!(!a.item_is_public_name("_Hidden.visible"));
        assert!(!a.item_is_public_name("_helper"));
    }

    #[test]
    fn pytest_file_patterns() {
        let a = PythonAdapter;
        assert!(a.is_test_file(Path::new("tests/test_packet.py")));
        assert!(a.is_test_file(Path::new("tests/packet_test.py")));
        assert!(!a.is_test_file(Path::new("tests/conftest.py")));
        assert!(!a.is_test_file(Path::new("src/pkg/testing.py")));
    }
}
