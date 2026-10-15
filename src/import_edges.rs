//! Internal edges: the imports between one package's own modules.
//!
//! Each language's `imports.scm` names the import forms (`@import.source`)
//! and re-exports (`@import.reexport`); the adapter turns a specifier into
//! candidate project files ([`LanguageAdapter::resolve_import`]). The first
//! candidate that exists is the import's target. A target file that belongs
//! to no spec module is followed only through its re-exports — that is how
//! a barrel (`index.ts`, an `export`-only Dart library) is seen through.
//! Anything that resolves to no project file is external: the manifest's
//! business, not the spec's: one package's edges only.
//!
//! Modules that share one source file are one node: their `depends_on`
//! lists are united, and the edges between them are not checked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::lang::{ImportContext, LanguageAdapter};
use crate::layout::LayoutContext;

/// One import in one file.
#[derive(Debug, Clone)]
pub struct FileImport {
    pub spec: String,
    /// Project-relative target file; `None` when external.
    pub resolved: Option<String>,
    pub reexport: bool,
    /// Inside code compiled only for tests (`@import.test_scope`): not a
    /// dependency of the module.
    pub test_only: bool,
}

/// One spec module, as the edge check needs it.
#[derive(Debug, Clone)]
pub struct ModuleNode {
    pub name: String,
    /// Project-relative source file, when it resolves.
    pub source: Option<String>,
    pub depends_on: Vec<String>,
}

/// An import whose target module the importing module does not declare.
#[derive(Debug, Clone)]
pub struct UndeclaredEdge {
    pub from: String,
    pub to: String,
    pub spec: String,
    pub file: String,
}

/// What the edge check found.
#[derive(Debug, Default)]
pub struct Edges {
    pub undeclared: Vec<UndeclaredEdge>,
    /// `(module, shared file, the other modules in it)`.
    pub shared: Vec<(String, String, Vec<String>)>,
    /// `false` when the language ships no `imports.scm`.
    pub checked: bool,
}

/// The import context for the project's own language.
pub fn context(layout: &LayoutContext, adapter: &dyn LanguageAdapter) -> ImportContext {
    ImportContext {
        project_root: layout.project_root.clone(),
        source_root: PathBuf::from(&layout.spec.paths.source),
        package_name: layout.spec.project.name.clone(),
        aliases: adapter.import_aliases(&layout.project_root),
    }
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

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// The imports of one project-relative file, each resolved.
pub fn file_imports(
    adapter: &dyn LanguageAdapter,
    ctx: &ImportContext,
    rel: &Path,
) -> Result<Vec<FileImport>> {
    let Some(query_src) = adapter.imports_query() else {
        return Ok(Vec::new());
    };
    let path = ctx.project_root.join(rel);
    let source =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let ext = rel.extension().and_then(|e| e.to_str()).unwrap_or("");
    let lang = adapter.tree_sitter_language_for(ext);
    let mut parser = Parser::new();
    parser
        .set_language(&lang)
        .context("setting the tree-sitter language")?;
    let Some(tree) = parser.parse(&source, None) else {
        return Ok(Vec::new());
    };
    let query = Query::new(&lang, query_src)
        .with_context(|| format!("compiling the {} imports.scm query", adapter.name()))?;
    let (c_src, c_re, c_scope) = (
        query.capture_index_for_name("import.source"),
        query.capture_index_for_name("import.reexport"),
        query.capture_index_for_name("import.test_scope"),
    );
    let mut found: Vec<(usize, usize, String, bool)> = Vec::new();
    let mut scopes: Vec<(usize, usize)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(m) = matches.next() {
        for c in m.captures {
            if Some(c.index) == c_scope {
                scopes.push((c.node.start_byte(), c.node.end_byte()));
                continue;
            }
            let reexport = Some(c.index) == c_re;
            if !(reexport || Some(c.index) == c_src) {
                continue;
            }
            let spec = strip_quotes(&source[c.node.byte_range()]).to_string();
            found.push((c.node.start_byte(), c.node.end_byte(), spec, reexport));
        }
    }
    // Nested captures of one path share its start byte (`crate::a` inside
    // `crate::a::B`, or a path inside a `use` line): one import, the longest.
    found.sort_by(|a, b| (a.0, a.3, b.1).cmp(&(b.0, b.3, a.1)));
    found.dedup_by(|later, kept| later.0 == kept.0 && later.3 == kept.3);
    let mut out = Vec::new();
    for (start, end, spec, reexport) in found {
        let resolved = adapter
            .resolve_import(&spec, rel, ctx)
            .into_iter()
            .find(|cand| ctx.project_root.join(cand).is_file())
            .map(|p| slash(&p));
        let test_only = scopes.iter().any(|(s, e)| *s <= start && end <= *e);
        out.push(FileImport {
            spec,
            resolved,
            reexport,
            test_only,
        });
    }
    Ok(out)
}

/// `report imports --root`: every file under `roots` with one of the
/// language's extensions, and its imports.
pub fn report_roots(
    adapter: &dyn LanguageAdapter,
    ctx: &ImportContext,
    roots: &[PathBuf],
) -> Result<Vec<(String, Vec<FileImport>)>> {
    let exts = adapter.file_extensions();
    let mut files = BTreeSet::new();
    for r in roots {
        walk(&ctx.project_root.join(r), &mut files);
    }
    let mut out = Vec::new();
    for abs in files {
        let ext = abs.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !exts.contains(&ext) {
            continue;
        }
        let rel = abs
            .strip_prefix(&ctx.project_root)
            .unwrap_or(&abs)
            .to_path_buf();
        let imports = file_imports(adapter, ctx, &rel)?;
        out.push((slash(&rel), imports));
    }
    Ok(out)
}

pub(crate) fn walk(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    if dir.is_file() {
        out.insert(dir.to_path_buf());
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let name = e.file_name();
            if ![
                "node_modules",
                "target",
                ".git",
                ".dart_tool",
                "build",
                "dist",
                ".next",
            ]
            .contains(&name.to_string_lossy().as_ref())
            {
                walk(&p, out);
            }
        } else {
            out.insert(p);
        }
    }
}

/// The internal edges of the project's modules.
pub fn module_edges(layout: &LayoutContext, modules: &[ModuleNode]) -> Result<Edges> {
    let Some(adapter) = crate::lang::adapter_for(&layout.spec.project.language) else {
        return Ok(Edges::default());
    };
    if adapter.imports_query().is_none() {
        return Ok(Edges::default());
    }
    let ctx = context(layout, adapter.as_ref());

    // File → the modules it belongs to (source file and its twins).
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut sources: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for m in modules {
        let Some(src) = &m.source else { continue };
        sources
            .entry(src.clone())
            .or_default()
            .insert(m.name.clone());
        owners
            .entry(src.clone())
            .or_default()
            .insert(m.name.clone());
        for twin in adapter.module_twin_files(Path::new(src)) {
            owners
                .entry(slash(&twin))
                .or_default()
                .insert(m.name.clone());
        }
    }
    let depends: BTreeMap<&str, &Vec<String>> = modules
        .iter()
        .map(|m| (m.name.as_str(), &m.depends_on))
        .collect();

    let mut edges = Edges {
        checked: true,
        ..Default::default()
    };
    let mut cache: BTreeMap<String, Vec<FileImport>> = BTreeMap::new();
    for (file, group) in &sources {
        if group.len() > 1 {
            for m in group {
                let others: Vec<String> = group.iter().filter(|o| *o != m).cloned().collect();
                edges.shared.push((m.clone(), file.clone(), others));
            }
        }
        let declared: BTreeSet<&str> = group
            .iter()
            .flat_map(|m| {
                depends
                    .get(m.as_str())
                    .into_iter()
                    .flat_map(|d| d.iter().map(String::as_str))
            })
            .collect();
        let from = group.iter().next().cloned().unwrap_or_default();
        let imports = imports_of(adapter.as_ref(), &ctx, file, &mut cache)?;
        let mut reported: BTreeSet<String> = BTreeSet::new();
        for imp in &imports {
            if imp.test_only {
                continue;
            }
            let Some(target) = &imp.resolved else {
                continue;
            };
            for to in target_modules(
                adapter.as_ref(),
                &ctx,
                target,
                &owners,
                &mut cache,
                &mut BTreeSet::new(),
            )? {
                if group.contains(&to)
                    || declared.contains(to.as_str())
                    || !reported.insert(to.clone())
                {
                    continue;
                }
                edges.undeclared.push(UndeclaredEdge {
                    from: from.clone(),
                    to,
                    spec: imp.spec.clone(),
                    file: file.clone(),
                });
            }
        }
    }
    Ok(edges)
}

fn imports_of(
    adapter: &dyn LanguageAdapter,
    ctx: &ImportContext,
    file: &str,
    cache: &mut BTreeMap<String, Vec<FileImport>>,
) -> Result<Vec<FileImport>> {
    if let Some(v) = cache.get(file) {
        return Ok(v.clone());
    }
    let v = file_imports(adapter, ctx, Path::new(file))?;
    cache.insert(file.to_string(), v.clone());
    Ok(v)
}

/// The modules a resolved target file stands for: its owners, or — for a
/// file no module owns — whatever its re-exports lead to.
fn target_modules(
    adapter: &dyn LanguageAdapter,
    ctx: &ImportContext,
    target: &str,
    owners: &BTreeMap<String, BTreeSet<String>>,
    cache: &mut BTreeMap<String, Vec<FileImport>>,
    seen: &mut BTreeSet<String>,
) -> Result<Vec<String>> {
    if let Some(o) = owners.get(target) {
        return Ok(o.iter().cloned().collect());
    }
    if !seen.insert(target.to_string()) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for imp in imports_of(adapter, ctx, target, cache)? {
        if !imp.reexport || imp.test_only {
            continue;
        }
        if let Some(next) = &imp.resolved {
            out.extend(target_modules(adapter, ctx, next, owners, cache, seen)?);
        }
    }
    Ok(out)
}

/// The `report imports --format yaml` document.
pub fn to_yaml(files: &[(String, Vec<FileImport>)]) -> Result<String> {
    let files: Vec<serde_json::Value> = files
        .iter()
        .map(|(file, imports)| {
            serde_json::json!({
                "file": file,
                "imports": imports.iter().map(|i| {
                    let mut v = serde_json::json!({
                        "spec": i.spec,
                        "resolved": i.resolved,
                        "reexport": i.reexport,
                    });
                    if i.test_only {
                        v["test_only"] = serde_json::json!(true);
                    }
                    v
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(serde_yaml::to_string(
        &serde_json::json!({ "files": files }),
    )?)
}
