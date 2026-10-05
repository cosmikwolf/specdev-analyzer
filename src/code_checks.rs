//! Spec ↔ code: the `[[code_checks]]` of each inlined rule pack.
//!
//! A code check names a query (inlined under `specs/specdev/queries/`), the
//! spec entity kind it is compared with, and its captures: the code key, and
//! optionally each argument's name and value. The runner runs the query over
//! every source file under the project's source root in the project's
//! language, groups the captured arguments by key, and compares them with
//! the spec's entities of that kind. A spec entity's code key is its name
//! without the owning module's prefix (`Ticker.on_tick` → `on_tick`).
//!
//! * `pack_entity_in_code` — a spec entity whose key the code does not
//!   have. Suppressed when the entity is planned (the entry, its unit table
//!   or its module).
//! * `pack_attribute_matches` — a mapped attribute whose code value differs
//!   from the spec's, including one side having it and the other not.
//!   Checked whenever the key is found, planned or not.
//! * `pack_key_ambiguous` — the code has the key at more than one place (two
//!   functions of one name in different files or modules). Nothing is
//!   merged: the attributes are not compared until one place remains.
//! * `stale_planned` — a planned entry whose key the code has. Not repeated
//!   for an entry planned only through its module: the module's own
//!   `stale_planned` says it.
//!
//! An entity whose module has no resolvable source is left to
//! `source_resolves`. The runner holds no unit knowledge: the query, the
//! kind and the mapping are the pack's data.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;
use tree_sitter::{Language, Parser, Query, QueryCursor, StreamingIterator};

use crate::domain::Severity;
use crate::graph::scalar_at;
use crate::lang::LanguageAdapter;
use crate::layout::LayoutContext;
use crate::packs::{CodeCheck, Pack};
use crate::report::{Finding, Report};

pub const ENTITY_IN_CODE: &str = "pack_entity_in_code";
pub const ATTRIBUTE_MATCHES: &str = "pack_attribute_matches";
pub const KEY_AMBIGUOUS: &str = "pack_key_ambiguous";
const STALE: &str = "stale_planned";
/// The finding code for an inlined query that does not compile or lacks a
/// capture its mapping names.
pub const QUERY_RULE: &str = "PACK-002";

/// One spec module, as the runner needs it.
pub struct ModuleView<'a> {
    pub name: &'a str,
    pub planned: bool,
    pub built: bool,
}

/// What the code holds for one key: where it is, and its arguments.
#[derive(Default)]
struct CodeEntry {
    /// `file:line` of each distinct key node. More than one is ambiguous.
    sites: BTreeSet<String>,
    /// Argument name → value.
    args: BTreeMap<String, String>,
}

/// Code key → its entry.
type CodeValues = BTreeMap<String, CodeEntry>;

/// Run every inlined pack's code checks for the project's language.
pub fn run(
    layout: &LayoutContext,
    graph: &Value,
    modules: &[ModuleView<'_>],
    project_rules: &BTreeMap<String, Severity>,
    report: &mut Report,
) {
    let language = layout.spec.project.language.as_str();
    let Some(adapter) = crate::lang::adapter_for(language) else { return };
    let (packs, _) = crate::packs::load_inlined(&layout.specdev_root); // load errors: validate's
    let checks: Vec<(&Pack, &CodeCheck)> = packs
        .iter()
        .flat_map(|p| p.file.code_checks.iter().map(move |c| (p, c)))
        .filter(|(_, c)| c.language == language)
        .collect();
    if checks.is_empty() {
        return;
    }
    let files = source_files(layout, adapter.as_ref());
    // The same rule, and default, as a planned module's stale flag.
    let stale_sev = project_rules.get(STALE).copied().unwrap_or(Severity::Warning);
    for (pack, check) in checks {
        let sev = |rule: &str| crate::packs::severity(project_rules, &pack.file.rules, rule);
        let code = match code_values(layout, adapter.as_ref(), pack, check, &files) {
            Ok(code) => code,
            Err(msg) => {
                report.push(Finding::new(QUERY_RULE, Severity::Error, msg).with_file(pack.path.display().to_string()));
                continue;
            }
        };
        compare(graph, modules, check, &code, &sev, stale_sev, report);
    }
}

/// Every file under the source root with one of the language's extensions.
fn source_files(layout: &LayoutContext, adapter: &dyn LanguageAdapter) -> Vec<PathBuf> {
    let exts = adapter.file_extensions();
    let mut all = BTreeSet::new();
    crate::import_edges::walk(&layout.project_root.join(&layout.spec.paths.source), &mut all);
    all.into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| exts.contains(&e)))
        .collect()
}

fn strip_quotes(s: &str) -> &str {
    let t = s.trim();
    for q in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(q) && t.ends_with(q) {
            return &t[1..t.len() - 1];
        }
    }
    t
}

/// Run the check's query over `files`; group the argument captures by key.
fn code_values(
    layout: &LayoutContext,
    adapter: &dyn LanguageAdapter,
    pack: &Pack,
    check: &CodeCheck,
    files: &[PathBuf],
) -> Result<CodeValues, String> {
    let qpath = pack
        .query_path(&layout.specdev_root, check)
        .ok_or_else(|| format!("rule pack {}: query path `{}` is not `languages/<lang>/<file>.scm`", pack.path.display(), check.query))?;
    let qsrc = std::fs::read_to_string(&qpath)
        .map_err(|e| format!("rule pack {}: cannot read query {}: {e}", pack.path.display(), qpath.display()))?;
    let mut queries: BTreeMap<String, (Language, Query)> = BTreeMap::new();
    let mut out = CodeValues::new();
    for file in files {
        let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
        if !queries.contains_key(&ext) {
            let lang = adapter.tree_sitter_language_for(&ext);
            let query = Query::new(&lang, &qsrc)
                .map_err(|e| format!("query {} does not compile for `{}`: {e}", qpath.display(), check.language))?;
            queries.insert(ext.clone(), (lang, query));
        }
        let (lang, query) = &queries[&ext];
        let idx = |n: &str| {
            query
                .capture_index_for_name(n)
                .ok_or_else(|| format!("query {} has no capture `@{n}`", qpath.display()))
        };
        let key_i = idx(&check.key_capture)?;
        let arg_i = check.arg_key_capture.as_deref().map(idx).transpose()?;
        let val_i = check.arg_value_capture.as_deref().map(idx).transpose()?;
        let Ok(source) = std::fs::read_to_string(file) else { continue };
        let mut parser = Parser::new();
        if parser.set_language(lang).is_err() {
            continue;
        }
        let Some(tree) = parser.parse(&source, None) else { continue };
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());
        while let Some(m) = matches.next() {
            let text = |i: Option<u32>| {
                i.and_then(|i| m.captures.iter().find(|c| c.index == i))
                    .map(|c| strip_quotes(&source[c.node.byte_range()]).to_string())
            };
            let Some(key_node) = m.captures.iter().find(|c| c.index == key_i).map(|c| c.node) else {
                continue;
            };
            let key = strip_quotes(&source[key_node.byte_range()]).to_string();
            let rel = file.strip_prefix(&layout.project_root).unwrap_or(file);
            let entry = out.entry(key).or_default();
            entry.sites.insert(format!("{}:{}", rel.display(), key_node.start_position().row + 1));
            if let (Some(k), Some(v)) = (text(arg_i), text(val_i)) {
                entry.args.insert(k, v);
            }
        }
    }
    Ok(out)
}

fn compare(
    graph: &Value,
    modules: &[ModuleView<'_>],
    check: &CodeCheck,
    code: &CodeValues,
    sev: &dyn Fn(&str) -> Severity,
    stale_sev: Severity,
    report: &mut Report,
) {
    for e in graph.get("entities").and_then(Value::as_array).into_iter().flatten() {
        if e.get("kind").and_then(Value::as_str) != Some(check.entity_kind.as_str()) {
            continue;
        }
        let Some(name) = e.get("name").and_then(Value::as_str) else { continue };
        let Some((module, key)) = modules
            .iter()
            .find_map(|m| name.strip_prefix(m.name).and_then(|r| r.strip_prefix('.')).map(|k| (m, k)))
        else {
            continue;
        };
        if !module.built {
            continue; // `source_resolves` owns a module with no source
        }
        let file = e.pointer("/provenance/source_file").and_then(Value::as_str).unwrap_or_default();
        let planned = e.pointer("/attrs/planned").and_then(Value::as_bool) == Some(true);
        let Some(entry) = code.get(key) else {
            if !planned {
                report.push(
                    Finding::new(
                        ENTITY_IN_CODE,
                        sev(ENTITY_IN_CODE),
                        format!("`{name}` ({}) is not in the code: no match for `{key}`", check.entity_kind),
                    )
                    .with_file(file)
                    .with_entity(name),
                );
            }
            continue;
        };
        if entry.sites.len() > 1 {
            let sites: Vec<&str> = entry.sites.iter().map(String::as_str).collect();
            report.push(
                Finding::new(
                    KEY_AMBIGUOUS,
                    sev(KEY_AMBIGUOUS),
                    format!(
                        "`{name}`: the code has `{key}` at {} places ({}); the check cannot tell which one the spec \
                         means, so its attributes are not compared",
                        sites.len(),
                        sites.join(", ")
                    ),
                )
                .with_file(file)
                .with_entity(name),
            );
            continue;
        }
        let args = &entry.args;
        if planned && !module.planned {
            report.push(
                Finding::new(
                    STALE,
                    stale_sev,
                    format!("`{name}` is planned, but the code has it; remove `planned`"),
                )
                .with_file(file)
                .with_entity(name),
            );
        }
        let diffs: Vec<String> = check
            .attributes
            .iter()
            .filter_map(|(arg, attr)| {
                let spec = scalar_at(e, &format!("attrs.{attr}"));
                let have = args.get(arg).cloned();
                (spec != have).then(|| {
                    format!("`{attr}` is {} in the spec, {} in the code", shown(&spec), shown(&have))
                })
            })
            .collect();
        if !diffs.is_empty() {
            report.push(
                Finding::new(ATTRIBUTE_MATCHES, sev(ATTRIBUTE_MATCHES), format!("`{name}`: {}", diffs.join("; ")))
                    .with_file(file)
                    .with_entity(name),
            );
        }
    }
}

fn shown(v: &Option<String>) -> String {
    v.as_deref().map(|s| format!("`{s}`")).unwrap_or_else(|| "absent".to_string())
}
