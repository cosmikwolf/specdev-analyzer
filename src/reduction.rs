//! Reduction executor — per-domain `reduction.yaml` → universal
//! `(entities, relations)` graph.
//!
//! A reduction spec describes how to map authored TOML specs under `specs/`
//! into an extend-only set of Entity and Relation records keyed by canonical
//! root/relation kinds (see `schema/meta/v1/`). Authored TOML is never
//! modified — this is a derived, read-only view.
//!
//! See `specdev-next/10-metaschema.md` for motivation and the deliberate
//! v1 cut line. This module implements steps 3–4 of that doc's seven-step
//! migration (metaschema + executor running alongside the existing
//! extractor, not replacing it).
//!
//! # Scoping rules (v1)
//!
//! * `parent.name` in a `for_each` scope resolves to the file-root's `name`
//!   field. Nested-parent references (`parent.parent.*`) are rejected.
//! * Attribute selectors that yield zero picks are omitted (absent ≠ null).
//! * Endpoint resolution is post-entity-materialization: relations whose
//!   `from`/`to` don't land on an existing `(kind, name)` emit a
//!   `REDUCE_UNRESOLVED` finding carrying TOML provenance.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::domain::Severity;
use crate::path::{eval_template, select, Scope};
use crate::report::{Finding, Provenance, Report};
use crate::schema::SchemaRegistry;

/// Parsed `reduction.yaml`. Matches the metaschema at
/// `schema/meta/v1/reduction.schema.yaml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ReductionSpec {
    pub metaschema_version: String,
    pub entities: Vec<EntityDecl>,
    pub relations: Vec<RelationDecl>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EntityDecl {
    pub kind: String,
    pub extends: String,
    pub from: String,
    pub name: String,
    #[serde(default)]
    pub for_each: Option<String>,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
    // `render` is ignored by the executor; metaschema validates shape.
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelationDecl {
    pub kind: String,
    pub from_kind: String,
    pub to_kind: String,
    pub from_spec: String,
    #[serde(default)]
    pub for_each: Option<String>,
    pub endpoints: Endpoints,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Endpoints {
    pub from: String,
    pub to: String,
}

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

/// Library-friendly entry point: resolve the project context, locate the
/// sibling `reduction.yaml` and plugin metaschema directory, validate the
/// spec, and execute it. Returns the graph plus any findings (unresolved
/// endpoints etc.).
pub fn run_project(project: &Path, plugin_root: Option<&Path>) -> Result<(Graph, Report)> {
    let ctx = crate::resolve::resolve(project, plugin_root)?;
    let reduction_file = ctx.domain_dir.path.join("reduction.yaml");
    if !reduction_file.is_file() {
        bail!(
            "reduction spec not found at {} — domain `{}` does not ship one",
            reduction_file.display(),
            ctx.domain
        );
    }
    let meta_dir = match &ctx.plugin_root {
        Some(root) => root.join("schema").join("meta").join("v1"),
        None => {
            bail!("cannot locate metaschema directory — pass plugin_root or set CLAUDE_PLUGIN_ROOT")
        }
    };
    let spec = load_reduction_spec(&reduction_file, &meta_dir)?;
    execute(&spec, &ctx.project_root)
}

/// Load and metaschema-validate a `reduction.yaml`.
pub fn load_reduction_spec(path: &Path, meta_dir: &Path) -> Result<ReductionSpec> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading reduction spec {}", path.display()))?;
    let value: Value = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing reduction spec {}", path.display()))?;

    let registry = SchemaRegistry::load_dir(meta_dir)
        .with_context(|| format!("loading metaschema from {}", meta_dir.display()))?;
    let findings = registry.validate("reduction", &value, &path.display().to_string())?;
    if !findings.is_empty() {
        let msgs: Vec<String> = findings.iter().map(|f| f.message.clone()).collect();
        bail!(
            "reduction spec {} failed metaschema:\n  - {}",
            path.display(),
            msgs.join("\n  - ")
        );
    }

    let spec: ReductionSpec = serde_yaml::from_str(&text)
        .with_context(|| format!("deserializing reduction spec {}", path.display()))?;
    Ok(spec)
}

/// Run the reduction. The returned `Report` captures unresolved-endpoint
/// findings (and any other semantic failures); the `Graph` is always
/// populated with whichever entities/relations did resolve so callers can
/// inspect partial output.
pub fn execute(spec: &ReductionSpec, project_root: &Path) -> Result<(Graph, Report)> {
    let mut graph = Graph::default();
    let mut report = Report::default();

    // ---- Pass 1: entities ----
    for decl in &spec.entities {
        let files = glob_from_root(project_root, &decl.from)?;
        for file in files {
            let rel_file = relpath(&file, project_root);
            let root_value = load_toml_as_json(&file)?;
            let parent_name = file_root_name(&root_value);

            let iterations = iterate(
                &root_value,
                decl.for_each.as_deref(),
                parent_name.as_deref(),
            );
            for iter in iterations {
                let scope = Scope {
                    root: &root_value,
                    current: Some(iter.current),
                    parent_name: iter.parent_name,
                };
                let name = eval_template(&decl.name, &scope).with_context(|| {
                    format!(
                        "evaluating entity `{}` name template in {}",
                        decl.kind, rel_file
                    )
                })?;

                let mut attrs = BTreeMap::new();
                for (k, selector) in &decl.attrs {
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
                    kind: decl.kind.clone(),
                    extends: decl.extends.clone(),
                    name,
                    attrs,
                    provenance: Provenance::new(rel_file.clone(), iter.pointer).into(),
                });
            }
        }
    }

    let mut index: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    for (i, e) in graph.entities.iter().enumerate() {
        index.insert((e.kind.clone(), e.name.clone()), i);
    }

    // ---- Pass 2: relations ----
    for decl in &spec.relations {
        let files = glob_from_root(project_root, &decl.from_spec)?;
        for file in files {
            let rel_file = relpath(&file, project_root);
            let root_value = load_toml_as_json(&file)?;
            let parent_name = file_root_name(&root_value);

            let iterations = iterate(
                &root_value,
                decl.for_each.as_deref(),
                parent_name.as_deref(),
            );
            for iter in iterations {
                let scope = Scope {
                    root: &root_value,
                    current: Some(iter.current),
                    parent_name: iter.parent_name,
                };
                let from_name = eval_template(&decl.endpoints.from, &scope).with_context(|| {
                    format!(
                        "evaluating relation `{}` endpoints.from in {}",
                        decl.kind, rel_file
                    )
                })?;
                let to_name = eval_template(&decl.endpoints.to, &scope).with_context(|| {
                    format!(
                        "evaluating relation `{}` endpoints.to in {}",
                        decl.kind, rel_file
                    )
                })?;

                let provenance = Provenance::new(rel_file.clone(), iter.pointer.clone());
                let mut unresolved = false;
                if !index.contains_key(&(decl.from_kind.clone(), from_name.clone())) {
                    report.push(
                        Finding::new(
                            "REDUCE_UNRESOLVED",
                            Severity::Error,
                            format!(
                                "unresolved {} endpoint `{from_name}` (relation `{}`)",
                                decl.from_kind, decl.kind
                            ),
                        )
                        .with_provenance(provenance.clone()),
                    );
                    unresolved = true;
                }
                if !index.contains_key(&(decl.to_kind.clone(), to_name.clone())) {
                    report.push(
                        Finding::new(
                            "REDUCE_UNRESOLVED",
                            Severity::Error,
                            format!(
                                "unresolved {} endpoint `{to_name}` (relation `{}`)",
                                decl.to_kind, decl.kind
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
                for (k, selector) in &decl.attrs {
                    let picks = select(iter.current, selector);
                    if let Some(p) = picks.into_iter().next() {
                        attrs.insert(k.clone(), p.value);
                    }
                }

                graph.relations.push(Relation {
                    kind: decl.kind.clone(),
                    from_kind: decl.from_kind.clone(),
                    to_kind: decl.to_kind.clone(),
                    from: from_name,
                    to: to_name,
                    attrs,
                    provenance: provenance.into(),
                });
            }
        }
    }

    Ok((graph, report))
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
