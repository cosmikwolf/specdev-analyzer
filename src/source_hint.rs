//! Which source files could hold a module whose source does not resolve.
//!
//! `source_resolves` reports a module with no resolvable source. When the
//! code exists under another path (flxs1's board tasks live in
//! `src/tasks/<x>.rs`, not at the module-name path), the module spec still
//! names things the code defines. This module ranks the source files by
//! how many of those names each one defines, so the finding can show the
//! evidence. It proposes; it never writes `source_file`.
//!
//! **Names** are the `name` of each entry of every top-level array of
//! tables in the module TOML — the TOML's shape, not a field the analyzer
//! knows (the analyzer holds no profile concept). **Defines** means an item of the language's items query
//! with that name, at any visibility (the profile's visibility floor hides
//! `pub(crate)` and private functions, which is exactly what task code
//! is). Files are not filtered for tests: `LanguageAdapter::is_test_file`
//! means "may hold tests" (true for every Rust file), and the ranking
//! shows its evidence anyway. A language with no items query gives no
//! candidates.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::lang::LanguageAdapter;

/// At most this many files are named in a finding.
const MAX_CANDIDATES: usize = 3;

/// The names a module spec gives its parts: `name` of each entry of every
/// top-level array of tables.
pub fn module_names(doc: &toml::Value) -> Vec<String> {
    let mut out = BTreeSet::new();
    if let Some(table) = doc.as_table() {
        for v in table.values() {
            for entry in v.as_array().into_iter().flatten() {
                if let Some(name) = entry.get("name").and_then(toml::Value::as_str) {
                    out.insert(name.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
}

/// Every item name each source file defines, at any visibility.
pub struct NameIndex {
    by_file: BTreeMap<PathBuf, BTreeSet<String>>,
}

impl NameIndex {
    /// Index every source file under `source_root`.
    pub fn build(project_root: &Path, source_root: &Path, adapter: &dyn LanguageAdapter) -> Self {
        let mut by_file = BTreeMap::new();
        let Some(query_text) = adapter.items_query() else {
            return NameIndex { by_file };
        };
        let exts = adapter.file_extensions();
        let mut files = BTreeSet::new();
        crate::import_edges::walk(source_root, &mut files);
        for path in files {
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if !exts.contains(&ext) {
                continue;
            }
            let rel = path
                .strip_prefix(project_root)
                .unwrap_or(&path)
                .to_path_buf();
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            let names = item_names(&source, ext, adapter, query_text);
            if !names.is_empty() {
                by_file.insert(rel, names);
            }
        }
        NameIndex { by_file }
    }

    /// Files that define at least one of `names`, most first (then by
    /// path), at most [`MAX_CANDIDATES`]: each with the names it defines.
    pub fn candidates(&self, names: &[String]) -> Vec<(PathBuf, Vec<String>)> {
        let mut ranked: Vec<(PathBuf, Vec<String>)> = self
            .by_file
            .iter()
            .filter_map(|(file, defined)| {
                let hits: Vec<String> = names
                    .iter()
                    .filter(|n| defined.contains(n.as_str()))
                    .cloned()
                    .collect();
                (!hits.is_empty()).then(|| (file.clone(), hits))
            })
            .collect();
        ranked.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));
        ranked.truncate(MAX_CANDIDATES);
        ranked
    }
}

/// The text a finding appends, or `None` when no file defines any name.
pub fn render(candidates: &[(PathBuf, Vec<String>)], total: usize) -> Option<String> {
    if candidates.is_empty() {
        return None;
    }
    let parts: Vec<String> = candidates
        .iter()
        .map(|(file, hits)| {
            let names: Vec<String> = hits.iter().map(|n| format!("`{n}`")).collect();
            format!(
                "`{}` defines {} of {} name(s) ({})",
                file.display().to_string().replace('\\', "/"),
                hits.len(),
                total,
                names.join(", ")
            )
        })
        .collect();
    Some(format!("candidates: {}", parts.join("; ")))
}

/// Every `<kind>.name` capture of the items query in `source`.
fn item_names(
    source: &str,
    ext: &str,
    adapter: &dyn LanguageAdapter,
    query_text: &str,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let language = adapter.tree_sitter_language_for(ext);
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return out;
    }
    let Some(tree) = parser.parse(source, None) else {
        return out;
    };
    let Ok(query) = Query::new(&language, query_text) else {
        return out;
    };
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        for c in m.captures {
            if names[c.index as usize].ends_with(".name") {
                if let Ok(text) = c.node.utf8_text(source.as_bytes()) {
                    out.insert(text.to_string());
                }
            }
        }
    }
    out
}
