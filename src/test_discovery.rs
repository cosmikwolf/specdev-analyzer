//! Test discovery: the tests that exist in the source, found by each
//! language's `tests.scm` query, independent of module source files, of
//! `mod` nesting and of any visibility floor.
//!
//! The query names the forms (`#[test]`, `it(...)`, `TEST(...)`, …); this
//! module only reads captures:
//!
//! * `@test` — the node that is one test. Several patterns may match the
//!   same node; their captures merge.
//! * `@test.name` — the static name; `@test.unnamed` marks a form with
//!   none (counted, named by its container).
//! * `@test.suite` — a suite part; the base name becomes `Suite.Name`.
//! * `@test.pending` — the test is switched off.
//! * `@group` / `@group.name` — an enclosing container (`mod`, `describe`,
//!   `group`). A container that holds at least one test is itself a name a
//!   `verified_by` entry may use.
//! * `@test_macro` — the name of a macro defined in the file that makes
//!   tests; `@test.macro` — the macro a `@test` node calls. Such a node is a
//!   test only when its macro is a `@test_macro` of the same file.
//!
//! When several matches name one test, the earliest name in the source wins.
//!
//! Identity: the bare name when it is unique among all discovered tests;
//! otherwise the language's [`TestIdentityStyle`] decides.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::lang::{LanguageAdapter, TestIdentityStyle};
use crate::layout::LayoutContext;

/// Directory names a walk never enters.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    ".dart_tool",
    "build",
    "dist",
    ".next",
];

/// One discovered test.
#[derive(Debug, Clone)]
pub struct DiscoveredTest {
    /// What a `verified_by` entry names to mean this test; `None` for an
    /// unnamed form.
    pub identity: Option<String>,
    /// Project-relative file.
    pub file: String,
    pub pending: bool,
    /// Enclosing container names, outer to inner.
    pub groups: Vec<String>,
    base: Option<String>,
}

/// Everything one discovery pass found.
#[derive(Debug, Default)]
pub struct Discovery {
    pub tests: Vec<DiscoveredTest>,
    /// Container names that hold at least one test.
    pub containers: BTreeSet<String>,
    /// `false` when the language ships no `tests.scm` (nothing is checked).
    pub checked: bool,
}

impl Discovery {
    /// The tests a `verified_by` name resolves to: those whose identity
    /// is the name, or every test under a container of that name.
    pub fn resolve(&self, name: &str) -> Vec<&DiscoveredTest> {
        let by_identity: Vec<&DiscoveredTest> = self
            .tests
            .iter()
            .filter(|t| t.identity.as_deref() == Some(name))
            .collect();
        if !by_identity.is_empty() {
            return by_identity;
        }
        if self.containers.contains(name) {
            return self
                .tests
                .iter()
                .filter(|t| t.groups.iter().any(|g| g == name))
                .collect();
        }
        Vec::new()
    }

    /// The qualified identities a bare `name` stands for when more than one
    /// test carries it: the candidates a `verified_by` entry must choose from.
    pub fn candidates(&self, name: &str) -> Vec<String> {
        self.tests
            .iter()
            .filter(|t| t.base.as_deref() == Some(name))
            .filter_map(|t| t.identity.clone())
            .filter(|i| i != name)
            .collect()
    }

    /// The finding text for a `verified_by` name that resolves to no test.
    pub fn unresolved_message(&self, owner: &str, name: &str) -> String {
        let c = self.candidates(name);
        if c.is_empty() {
            format!("`{owner}`: `verified_by` names `{name}`, which is no discovered test")
        } else {
            format!(
                "`{owner}`: `verified_by` names `{name}`, which is more than one test; name one of: {}",
                c.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ")
            )
        }
    }
}

/// Discover the project's tests: `[paths].source`, `[paths].tests`, and
/// the language's conventional test roots.
pub fn discover_project(layout: &LayoutContext) -> Result<Discovery> {
    let Some(adapter) = crate::lang::adapter_for(&layout.spec.project.language) else {
        return Ok(Discovery::default());
    };
    let mut roots: Vec<PathBuf> = vec![layout.source_dir.clone()];
    roots.extend(
        layout
            .spec
            .paths
            .tests
            .iter()
            .map(|t| layout.code_root.join(t)),
    );
    roots.extend(
        adapter
            .conventional_test_roots()
            .iter()
            .map(|r| layout.code_root.join(r)),
    );
    let modules = module_files(layout, adapter.as_ref());
    discover(adapter.as_ref(), &layout.code_root, &roots, &modules)
}

/// Discover the tests under explicit `roots` (`report tests --root`).
pub fn discover_roots(
    adapter: &dyn LanguageAdapter,
    project_root: &Path,
    roots: &[PathBuf],
) -> Result<Discovery> {
    discover(adapter, project_root, roots, &BTreeMap::new())
}

/// Spec module name for each project-relative source file, from the same
/// resolution `check` uses.
fn module_files(layout: &LayoutContext, adapter: &dyn LanguageAdapter) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(rd) = std::fs::read_dir(&layout.modules_dir) else {
        return out;
    };
    let mut files: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    for f in files {
        let Ok(text) = std::fs::read_to_string(&f) else {
            continue;
        };
        let Ok(doc) = text.parse::<toml::Table>() else {
            continue;
        };
        let doc = toml::Value::Table(doc);
        let Some(name) = doc.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        if let Some(src) = crate::items::source_map::resolve_source_path(
            name,
            crate::items::source_map::source_file_override(&doc),
            adapter,
            &layout.code_root,
            &layout.spec.paths.source,
        ) {
            out.entry(rel(&layout.code_root, &src))
                .or_insert_with(|| name.to_string());
        }
    }
    out
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

fn walk(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) {
                walk(&p, out);
            }
        } else {
            out.insert(p);
        }
    }
}

struct RawTest {
    start: usize,
    end: usize,
    name: Option<String>,
    /// Byte offset of `name`: the earliest name in the source wins.
    name_at: usize,
    suite: Option<String>,
    named: bool,
    unnamed: bool,
    pending: bool,
}

fn discover(
    adapter: &dyn LanguageAdapter,
    project_root: &Path,
    roots: &[PathBuf],
    module_of_file: &BTreeMap<String, String>,
) -> Result<Discovery> {
    let Some(query_src) = adapter.tests_query() else {
        return Ok(Discovery::default());
    };
    let exts = adapter.file_extensions();
    let mut files = BTreeSet::new();
    for r in roots {
        if r.is_file() {
            files.insert(r.clone());
        } else {
            walk(r, &mut files);
        }
    }
    let mut disc = Discovery {
        checked: true,
        ..Default::default()
    };
    for path in files {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !exts.contains(&ext) {
            continue;
        }
        let rel_path = rel(project_root, &path);
        if !adapter.is_test_file(Path::new(&rel_path)) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let lang = adapter.tree_sitter_language_for(ext);
        let mut parser = Parser::new();
        parser
            .set_language(&lang)
            .context("setting the tree-sitter language")?;
        let Some(tree) = parser.parse(&source, None) else {
            continue;
        };
        let query = Query::new(&lang, query_src)
            .with_context(|| format!("compiling the {} tests.scm query", adapter.name()))?;
        let idx = |n: &str| query.capture_index_for_name(n);
        let (c_test, c_name, c_suite, c_unnamed, c_pending, c_group, c_group_name) = (
            idx("test"),
            idx("test.name"),
            idx("test.suite"),
            idx("test.unnamed"),
            idx("test.pending"),
            idx("group"),
            idx("group.name"),
        );
        let (c_test_macro, c_macro_call) = (idx("test_macro"), idx("test.macro"));
        let mut test_macros: BTreeSet<String> = BTreeSet::new();
        // Calls of a macro, kept apart until the file's test macros are known.
        let mut calls: BTreeMap<(usize, usize), (String, RawTest)> = BTreeMap::new();
        let text = |n: tree_sitter::Node| strip_quotes(&source[n.byte_range()]).to_string();
        let mut tests: BTreeMap<(usize, usize), RawTest> = BTreeMap::new();
        let mut groups: Vec<(usize, usize, String)> = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            let node_of = |i: Option<u32>| {
                i.and_then(|i| m.captures.iter().find(|c| c.index == i).map(|c| c.node))
            };
            if let (Some(g), Some(gn)) = (node_of(c_group), node_of(c_group_name)) {
                groups.push((g.start_byte(), g.end_byte(), text(gn)));
            }
            if let Some(d) = node_of(c_test_macro) {
                test_macros.insert(text(d));
            }
            let Some(t) = node_of(c_test) else { continue };
            let blank = || RawTest {
                start: t.start_byte(),
                end: t.end_byte(),
                name: None,
                name_at: usize::MAX,
                suite: None,
                named: false,
                unnamed: false,
                pending: false,
            };
            let entry = match node_of(c_macro_call) {
                Some(mc) => {
                    &mut calls
                        .entry((t.start_byte(), t.end_byte()))
                        .or_insert_with(|| (text(mc), blank()))
                        .1
                }
                None => tests
                    .entry((t.start_byte(), t.end_byte()))
                    .or_insert_with(blank),
            };
            if let Some(n) = node_of(c_name) {
                if n.start_byte() < entry.name_at {
                    entry.name = Some(text(n));
                    entry.name_at = n.start_byte();
                }
                entry.named = true;
            }
            if let Some(s) = node_of(c_suite) {
                entry.suite = Some(text(s));
            }
            if node_of(c_unnamed).is_some() {
                entry.unnamed = true;
            }
            if node_of(c_pending).is_some() {
                entry.pending = true;
            }
        }
        for (key, (mac, call)) in calls {
            if test_macros.contains(&mac) && !tests.contains_key(&key) {
                tests.insert(key, call);
            }
        }
        groups.sort();
        for t in tests.into_values().filter(|t| t.named || t.unnamed) {
            let enclosing: Vec<String> = groups
                .iter()
                .filter(|(s, e, _)| *s <= t.start && t.end <= *e && !(*s == t.start && *e == t.end))
                .map(|(_, _, n)| n.clone())
                .collect();
            for g in &enclosing {
                disc.containers.insert(g.clone());
            }
            let base = if t.named {
                t.name.map(|n| match &t.suite {
                    Some(s) => format!("{s}.{n}"),
                    None => n,
                })
            } else {
                None
            };
            disc.tests.push(DiscoveredTest {
                identity: None,
                file: rel_path.clone(),
                pending: t.pending,
                groups: enclosing,
                base,
            });
        }
    }
    assign_identities(&mut disc, adapter.test_identity_style(), module_of_file);
    Ok(disc)
}

/// Whether the source file `rel` (relative to `code_root`) holds only tests: at
/// least one test, and every top-level node either inert (a comment, an
/// attribute, an import — [`LanguageAdapter::is_inert_top_level`]) or inside
/// a test or test group. `false` when the language ships no `tests.scm`.
pub fn holds_only_tests(
    adapter: &dyn LanguageAdapter,
    code_root: &Path,
    rel: &Path,
) -> Result<bool> {
    let Some(query_src) = adapter.tests_query() else {
        return Ok(false);
    };
    let path = code_root.join(rel);
    let source =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
    let lang = adapter.tree_sitter_language_for(ext);
    let mut parser = Parser::new();
    parser
        .set_language(&lang)
        .context("setting the tree-sitter language")?;
    let Some(tree) = parser.parse(&source, None) else {
        return Ok(false);
    };
    let query = Query::new(&lang, query_src)
        .with_context(|| format!("compiling the {} tests.scm query", adapter.name()))?;
    let (c_test, c_group, c_test_macro, c_macro_call) = (
        query.capture_index_for_name("test"),
        query.capture_index_for_name("group"),
        query.capture_index_for_name("test_macro"),
        query.capture_index_for_name("test.macro"),
    );
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut tests = 0;
    let mut test_macros: BTreeSet<String> = BTreeSet::new();
    let mut calls: BTreeMap<(usize, usize), String> = BTreeMap::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        let call = m.captures.iter().find(|c| Some(c.index) == c_macro_call);
        for c in m.captures {
            if Some(c.index) == c_test_macro {
                test_macros.insert(source[c.node.byte_range()].to_string());
            }
            if Some(c.index) == c_test {
                if let Some(mc) = call {
                    // A macro call counts only once its macro is known to make tests.
                    calls.insert(
                        (c.node.start_byte(), c.node.end_byte()),
                        source[mc.node.byte_range()].to_string(),
                    );
                    continue;
                }
                tests += 1;
            }
            if Some(c.index) == c_test || Some(c.index) == c_group {
                spans.push((c.node.start_byte(), c.node.end_byte()));
            }
        }
    }
    for ((start, end), mac) in calls {
        if test_macros.contains(&mac) {
            tests += 1;
            spans.push((start, end));
        }
    }
    if tests == 0 {
        return Ok(false);
    }
    let root = tree.root_node();
    let mut walker = root.walk();
    let only = root.named_children(&mut walker).all(|n| {
        adapter.is_inert_top_level(n.kind())
            || spans
                .iter()
                .any(|(s, e)| *s <= n.start_byte() && n.end_byte() <= *e)
    });
    Ok(only)
}

fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    for q in ['\'', '"', '`'] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return &t[1..t.len() - 1];
        }
    }
    t
}

fn assign_identities(
    disc: &mut Discovery,
    style: TestIdentityStyle,
    module_of_file: &BTreeMap<String, String>,
) {
    let mut count: BTreeMap<String, usize> = BTreeMap::new();
    for t in &disc.tests {
        if let Some(b) = &t.base {
            *count.entry(b.clone()).or_default() += 1;
        }
    }
    for t in &mut disc.tests {
        let Some(base) = t.base.clone() else { continue };
        t.identity = Some(if count[&base] == 1 {
            base
        } else {
            match style {
                TestIdentityStyle::GroupPath(sep) => {
                    let mut parts = t.groups.clone();
                    parts.push(base);
                    parts.join(sep)
                }
                TestIdentityStyle::FileGroupPath(sep) => {
                    let mut parts = vec![t.file.clone()];
                    parts.extend(t.groups.iter().cloned());
                    parts.push(base);
                    parts.join(sep)
                }
                TestIdentityStyle::ModuleOrFile => match module_of_file.get(&t.file) {
                    Some(m) => format!("{m}::{base}"),
                    None => format!("{}::{base}", t.file),
                },
            }
        });
    }
}

/// The `report tests --format yaml` document.
pub fn to_yaml(disc: &Discovery) -> Result<String> {
    let tests: Vec<serde_json::Value> = disc
        .tests
        .iter()
        .map(|t| {
            serde_json::json!({
                "identity": t.identity,
                "file": t.file,
                "pending": t.pending,
                "groups": t.groups,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "checked": disc.checked,
        "tests": tests,
        "containers": disc.containers,
    });
    Ok(serde_yaml::to_string(&doc)?)
}
