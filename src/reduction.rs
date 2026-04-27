//! Reduction executor — open-vocabulary `(entities, relations)` graph.
//!
//! Phase 2 rewires the executor to drive off the [`EffectiveKindSet`]
//! produced by [`crate::reductions::resolve`], rather than a closed-enum
//! `ReductionSpec`. Entity and Relation records are keyed by *qualified*
//! kind paths (e.g. `embassy.Task`, `embassy+lvgl.renders`). Cross-domain
//! dispatch happens through the `extends` chain on each record.
//!
//! The doc-10 §"Scoping rules (v1)" semantics are preserved verbatim:
//!
//! * `parent.name` in a `for_each` scope resolves to the file-root's `name`
//!   field. Nested-parent references (`parent.parent.*`) are rejected.
//! * Attribute selectors that yield zero picks are omitted (absent ≠ null).
//! * Endpoint resolution is post-entity-materialization: relations whose
//!   `from`/`to` don't land on an existing `(qualified-kind, name)` emit a
//!   `REDUCE_UNRESOLVED` finding carrying TOML provenance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::domain::Severity;
use crate::path::{eval_template, select, Scope};
use crate::report::{Finding, Provenance, Report};
use crate::resolved::{EffectiveKindSet, ResolvedEntityDecl, ResolvedRelationDecl};

#[derive(Debug, Clone, Serialize)]
pub struct Entity {
    pub kind: String,
    pub extends: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, Value>,
    pub provenance: SerializedProvenance,
}

#[derive(Debug, Clone, Serialize)]
pub struct Relation {
    pub kind: String,
    pub from_kind: String,
    pub to_kind: String,
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attrs: BTreeMap<String, Value>,
    pub provenance: SerializedProvenance,
}

/// Provenance in a form that serializes nicely as YAML.
#[derive(Debug, Clone, Serialize)]
pub struct SerializedProvenance {
    pub source_file: String,
    pub pointer: String,
}

impl From<Provenance> for SerializedProvenance {
    fn from(p: Provenance) -> Self {
        Self {
            source_file: p.source_file,
            pointer: p.pointer,
        }
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Graph {
    pub entities: Vec<Entity>,
    pub relations: Vec<Relation>,
}

/// Library-friendly entry point. Discovers the layout, loads + resolves
/// the project's reductions, and runs the executor against the resulting
/// [`EffectiveKindSet`]. The returned `Report` carries every finding from
/// the resolver pass plus any executor-level `REDUCE_UNRESOLVED` errors.
pub fn run_project(start: &Path, explicit_spec_path: Option<&Path>) -> Result<(Graph, Report)> {
    let layout = crate::layout::LayoutContext::discover(start, explicit_spec_path)?;
    let inv = crate::reductions::load_all(&layout)?;
    let (kindset, kindset_findings) = crate::reductions::resolve(&inv);
    let mut report = Report::default();
    report.extend(kindset_findings);
    let graph = execute_kindset(&kindset, &layout.project_root, &mut report)?;
    Ok((graph, report))
}

/// Materialize the project graph from a resolved [`EffectiveKindSet`] and
/// the project root. Open-vocabulary entry point — entity and relation
/// records are keyed by qualified kind paths (e.g. `embassy.Task`,
/// `embassy+lvgl.renders`), with `extends` carrying the canonical anchor
/// the qualified kind classifies under.
pub fn execute_kindset(
    kindset: &EffectiveKindSet,
    project_root: &Path,
    report: &mut Report,
) -> Result<Graph> {
    let mut graph = Graph::default();

    // ---- Pass 1: entities ----
    for (qualified_path, decl) in &kindset.entities {
        materialize_entity(
            qualified_path,
            decl,
            kindset,
            project_root,
            &mut graph,
            report,
        )?;
    }

    let mut index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    for (i, e) in graph.entities.iter().enumerate() {
        index.insert((e.kind.clone(), e.name.clone()), i);
    }

    // ---- Pass 2: relations ----
    for (qualified_path, decl) in &kindset.relations {
        materialize_relation(
            qualified_path,
            decl,
            project_root,
            &index,
            &mut graph,
            report,
        )?;
    }

    Ok(graph)
}

fn materialize_entity(
    qualified_path: &str,
    decl: &ResolvedEntityDecl,
    kindset: &EffectiveKindSet,
    project_root: &Path,
    graph: &mut Graph,
    _report: &mut Report,
) -> Result<()> {
    let body = decl_body(&decl.raw);
    let from = body
        .get("from")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name_template = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let for_each = body
        .get("for_each")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let attrs_map = body
        .get("attrs")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let canonical_root = kindset
        .entity_canonical_root(qualified_path)
        .unwrap_or_else(|| "structural".to_string());

    if from.is_empty() {
        return Ok(());
    }
    let files = glob_from_root(project_root, &from)?;
    for file in files {
        let rel_file = relpath(&file, project_root);
        let root_value = load_toml_as_json(&file)?;
        let parent_name = file_root_name(&root_value);

        let iterations = iterate(&root_value, for_each.as_deref(), parent_name.as_deref());
        for iter in iterations {
            let scope = Scope {
                root: &root_value,
                current: Some(iter.current),
                parent_name: iter.parent_name,
            };
            let name = eval_template(&name_template, &scope).with_context(|| {
                format!("evaluating entity `{qualified_path}` name template in {rel_file}")
            })?;

            let mut attrs = BTreeMap::new();
            for (k, selector_value) in &attrs_map {
                let Some(selector) = selector_value.as_str() else {
                    continue;
                };
                let picks = select(iter.current, selector);
                match picks.len() {
                    0 => {}
                    1 => {
                        attrs.insert(k.clone(), picks[0].value.clone());
                    }
                    _ => {
                        let arr: Vec<Value> = picks.into_iter().map(|p| p.value).collect();
                        attrs.insert(k.clone(), Value::Array(arr));
                    }
                }
            }

            graph.entities.push(Entity {
                kind: qualified_path.to_string(),
                extends: canonical_root.clone(),
                name,
                attrs,
                provenance: Provenance::new(rel_file.clone(), iter.pointer).into(),
            });
        }
    }
    Ok(())
}

fn materialize_relation(
    qualified_path: &str,
    decl: &ResolvedRelationDecl,
    project_root: &Path,
    index: &std::collections::HashMap<(String, String), usize>,
    graph: &mut Graph,
    report: &mut Report,
) -> Result<()> {
    let body = decl_body(&decl.raw);
    let from_spec = body
        .get("from_spec")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let for_each = body
        .get("for_each")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let endpoints = body.get("endpoints").and_then(|v| v.as_object()).cloned();
    let endpoints_from = endpoints
        .as_ref()
        .and_then(|e| e.get("from").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let endpoints_to = endpoints
        .as_ref()
        .and_then(|e| e.get("to").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let attrs_map = body
        .get("attrs")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();

    let from_kind_str = decl.from_kind.to_string();
    let to_kind_str = decl.to_kind.to_string();

    if from_spec.is_empty() {
        return Ok(());
    }
    let files = glob_from_root(project_root, &from_spec)?;
    for file in files {
        let rel_file = relpath(&file, project_root);
        let root_value = load_toml_as_json(&file)?;
        let parent_name = file_root_name(&root_value);

        let iterations = iterate(&root_value, for_each.as_deref(), parent_name.as_deref());
        for iter in iterations {
            let scope = Scope {
                root: &root_value,
                current: Some(iter.current),
                parent_name: iter.parent_name,
            };
            let from_name = eval_template(&endpoints_from, &scope).with_context(|| {
                format!("evaluating relation `{qualified_path}` endpoints.from in {rel_file}")
            })?;
            let to_name = eval_template(&endpoints_to, &scope).with_context(|| {
                format!("evaluating relation `{qualified_path}` endpoints.to in {rel_file}")
            })?;

            let provenance = Provenance::new(rel_file.clone(), iter.pointer.clone());
            let mut unresolved = false;
            if !index.contains_key(&(from_kind_str.clone(), from_name.clone())) {
                report.push(
                    Finding::new(
                        "REDUCE_UNRESOLVED",
                        Severity::Error,
                        format!(
                            "unresolved {from_kind_str} endpoint `{from_name}` (relation `{qualified_path}`)"
                        ),
                    )
                    .with_provenance(provenance.clone()),
                );
                unresolved = true;
            }
            if !index.contains_key(&(to_kind_str.clone(), to_name.clone())) {
                report.push(
                    Finding::new(
                        "REDUCE_UNRESOLVED",
                        Severity::Error,
                        format!(
                            "unresolved {to_kind_str} endpoint `{to_name}` (relation `{qualified_path}`)"
                        ),
                    )
                    .with_provenance(provenance.clone()),
                );
                unresolved = true;
            }
            if unresolved {
                continue;
            }

            let mut attrs = BTreeMap::new();
            for (k, selector_value) in &attrs_map {
                let Some(selector) = selector_value.as_str() else {
                    continue;
                };
                let picks = select(iter.current, selector);
                if let Some(p) = picks.into_iter().next() {
                    attrs.insert(k.clone(), p.value);
                }
            }

            graph.relations.push(Relation {
                kind: qualified_path.to_string(),
                from_kind: from_kind_str.clone(),
                to_kind: to_kind_str.clone(),
                from: from_name,
                to: to_name,
                attrs,
                provenance: provenance.into(),
            });
        }
    }
    Ok(())
}

/// Translate the YAML body retained on a [`ResolvedEntityDecl`] /
/// [`ResolvedRelationDecl`] into a JSON map for selector evaluation. The
/// YAML carries the verbatim `from`/`name`/`attrs`/`endpoints` fields.
fn decl_body(raw: &serde_yaml::Value) -> serde_json::Map<String, Value> {
    let json: Value = match serde_json::to_value(raw) {
        Ok(v) => v,
        Err(_) => return serde_json::Map::new(),
    };
    json.as_object().cloned().unwrap_or_default()
}

/// Deterministic YAML rendering for graphs. Keys are already canonical
/// on our structs (serde preserves declaration order), and BTreeMap-backed
/// attrs serialize alphabetically.
pub fn to_yaml(graph: &Graph) -> Result<String> {
    serde_yaml::to_string(graph).context("serializing reduction graph to YAML")
}

/// One iteration step produced by `iterate()`.
struct Iter<'a> {
    current: &'a Value,
    parent_name: Option<&'a str>,
    pointer: String,
}

fn iterate<'a>(
    root: &'a Value,
    for_each: Option<&str>,
    parent_name: Option<&'a str>,
) -> Vec<Iter<'a>> {
    match for_each {
        None => vec![Iter {
            current: root,
            parent_name: None,
            pointer: String::new(),
        }],
        Some(sel) => {
            let picks = select(root, sel);
            picks
                .into_iter()
                .filter_map(|p| {
                    // Have to re-borrow from root: `Pick` carries an owned
                    // clone of the value, but we want a reference to the
                    // original so Scope can outlive this closure.
                    let current = resolve_pointer(root, &p.pointer)?;
                    Some(Iter {
                        current,
                        parent_name,
                        pointer: p.pointer,
                    })
                })
                .collect()
        }
    }
}

/// Walk a JSON Pointer (per RFC 6901) against `root` and return the matching
/// borrowed sub-value. We only consume pointers produced by `path::select`,
/// so only the subset of `/name` + `/N` segments matters.
fn resolve_pointer<'a>(root: &'a Value, ptr: &str) -> Option<&'a Value> {
    if ptr.is_empty() {
        return Some(root);
    }
    let mut cur = root;
    for raw in ptr.split('/').skip(1) {
        let seg = raw.replace("~1", "/").replace("~0", "~");
        cur = match cur {
            Value::Object(map) => map.get(&seg)?,
            Value::Array(arr) => {
                let i: usize = seg.parse().ok()?;
                arr.get(i)?
            }
            _ => return None,
        };
    }
    Some(cur)
}

/// File-root `name` field, if the top-level TOML object declares one.
fn file_root_name(root: &Value) -> Option<String> {
    root.get("name").and_then(Value::as_str).map(str::to_string)
}

/// Glob files relative to the project root. Unlike `std::fs::read_dir`, this
/// supports the `*` / `**` patterns the metaschema's `from` / `from_spec`
/// fields allow. Non-matching globs return an empty list; I/O errors bail.
fn glob_from_root(project_root: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let full = project_root.join(pattern);
    let rendered = full
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-UTF-8 glob path under {}", project_root.display()))?;
    let mut out = Vec::new();
    for entry in glob::glob(rendered)
        .with_context(|| format!("invalid glob pattern `{pattern}`"))?
        .flatten()
    {
        if entry.is_file() {
            out.push(entry);
        }
    }
    out.sort();
    Ok(out)
}

fn relpath(full: &Path, root: &Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .replace('\\', "/")
}

fn load_toml_as_json(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing TOML {}", path.display()))?;
    serde_json::to_value(&raw)
        .with_context(|| format!("converting TOML to JSON for {}", path.display()))
}

/// Canonicalize a JSON value's keys (for stable YAML output). Present here
/// so callers that want byte-stable graph serialization don't have to reach
/// into `extract`.
#[allow(dead_code)]
pub(crate) fn canonicalize(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted: BTreeMap<String, Value> = BTreeMap::new();
            for (k, v) in map {
                sorted.insert(k, canonicalize(v));
            }
            let mut out = Map::new();
            for (k, v) in sorted {
                out.insert(k, v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize).collect()),
        other => other,
    }
}
