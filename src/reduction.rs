//! Reduction executor — open-vocabulary `(entities, relations)` graph.
//!
//! The executor drives off the [`EffectiveKindSet`]
//! produced by [`crate::reductions::resolve`], rather than a closed-enum
//! `ReductionSpec`. Entity and Relation records are keyed by *qualified*
//! kind paths (e.g. `embassy.Task`, `embassy+lvgl.renders`). Cross-domain
//! dispatch happens through the `extends` chain on each record.
//!
//! The scoping rules (v1) semantics are preserved verbatim:
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
use crate::layout::LayoutContext;
use crate::manifest::BridgeContract;
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
    /// Canonical relation kind the `kind` classifies under (`depends_on`,
    /// `contains`, …). Consumers that bucket relations by meaning read
    /// this instead of re-walking the kind set.
    pub extends: String,
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
    let graph = execute_kindset(&kindset, &layout, &mut report)?;
    Ok((graph, report))
}

/// Materialize the project graph from a resolved [`EffectiveKindSet`] and
/// the project [`LayoutContext`]. Open-vocabulary entry point — entity and
/// relation records are keyed by qualified kind paths (e.g. `embassy.Task`,
/// `embassy+lvgl.renders`), with `extends` carrying the canonical anchor
/// the qualified kind classifies under.
///
/// The layout (rather than just the project root) is threaded through so
/// the bridge-aware `REDUCE_UNRESOLVED` hint can locate inlined bridge
/// `contract.yaml` manifests under `<reductions_dir>/bridges/`. Pre-manifest
/// projects without inlined manifests fall back to the legacy generic
/// hint string.
pub fn execute_kindset(
    kindset: &EffectiveKindSet,
    layout: &LayoutContext,
    report: &mut Report,
) -> Result<Graph> {
    let project_root = layout.project_root.as_path();
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
    // Per-execution cache of bridge contract loads keyed by qualified-path
    // head (e.g. `egui+nih-plug`). Two known bridges in v1; the cache
    // saves redundant YAML reads when many relations under the same
    // bridge fail in the same project.
    let mut bridge_contract_cache: std::collections::HashMap<String, Option<BridgeContract>> =
        std::collections::HashMap::new();
    for (qualified_path, decl) in &kindset.relations {
        materialize_relation(
            qualified_path,
            decl,
            kindset,
            layout,
            &index,
            &mut graph,
            report,
            &mut bridge_contract_cache,
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

        let iterations = iterate(&root_value, for_each.as_deref());
        for iter in iterations {
            let scope = Scope {
                root: &root_value,
                current: Some(iter.current),
                parent: iter.parent,
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

#[allow(clippy::too_many_arguments)]
fn materialize_relation(
    qualified_path: &str,
    decl: &ResolvedRelationDecl,
    kindset: &EffectiveKindSet,
    layout: &LayoutContext,
    index: &std::collections::HashMap<(String, String), usize>,
    graph: &mut Graph,
    report: &mut Report,
    bridge_contract_cache: &mut std::collections::HashMap<String, Option<BridgeContract>>,
) -> Result<()> {
    let project_root = layout.project_root.as_path();
    let body = decl_body(&decl.raw);
    let canonical = kindset
        .relation_canonical(qualified_path)
        .unwrap_or_else(|| "depends_on".to_string());
    if let Some(m) = body.get("match").and_then(|v| v.as_object()) {
        materialize_match_relation(qualified_path, decl, &canonical, m, project_root, graph);
        return Ok(());
    }
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
        let parent_name_for_hint = file_root_name(&root_value);

        let iterations = iterate(&root_value, for_each.as_deref());
        for iter in iterations {
            let scope = Scope {
                root: &root_value,
                current: Some(iter.current),
                parent: iter.parent,
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
                // Source-side endpoint missing — rare; usually means
                // the file root's name field doesn't match a declared
                // entity. Frame the message so the unknown name is the
                // attention-grabber.
                let mut msg = format!(
                    "unknown {from_kind_str} `{from_name}` (source endpoint of relation \
                     `{qualified_path}` pointing at {to_kind_str} `{to_name}`)"
                );
                let hint = render_bridge_hint(
                    qualified_path,
                    &from_name,
                    &to_kind_str,
                    &to_name,
                    parent_name_for_hint.as_deref(),
                    layout,
                    &graph.entities,
                    project_root,
                    &provenance,
                    bridge_contract_cache,
                    report,
                );
                msg.push_str(&hint);
                report.push(
                    Finding::new("REDUCE_UNRESOLVED", Severity::Error, msg)
                        .with_provenance(provenance.clone()),
                );
                unresolved = true;
            }
            if !index.contains_key(&(to_kind_str.clone(), to_name.clone())) {
                // Target-side endpoint missing — the common "unknown
                // reference" case. Frame the message so a reader sees
                // the source entity (which holds the bad reference)
                // followed by the unknown target.
                let mut msg = format!(
                    "{from_kind_str} `{from_name}` references unknown {to_kind_str} \
                     `{to_name}` (relation `{qualified_path}`)"
                );
                let hint = render_bridge_hint(
                    qualified_path,
                    &to_name,
                    &from_kind_str,
                    &from_name,
                    parent_name_for_hint.as_deref(),
                    layout,
                    &graph.entities,
                    project_root,
                    &provenance,
                    bridge_contract_cache,
                    report,
                );
                msg.push_str(&hint);
                report.push(
                    Finding::new("REDUCE_UNRESOLVED", Severity::Error, msg)
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
                extends: canonical.clone(),
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

/// Render the bridge-aware hint line appended
/// to a `REDUCE_UNRESOLVED` finding when the relation is bridge-qualified
/// (`<a>+<b>.<rel>`). The message body comes from the
/// bridge's inlined `contract.yaml` `diagnostic_template`, with closed-set
/// `{bridge_name}` / `{parameter}` / `{decl_module}` / `{binding_module}`
/// placeholders substituted from the unresolved-relation context.
///
/// **Pre-manifest backward-compat:** when the contract.yaml is absent (older
/// inlined trees, hand-authored project-local bridges), fall back to the
/// generic hint string verbatim.
///
/// **Token-unresolvable fallback:** unresolvable tokens render as
/// `<unknown>` in the substituted output AND emit a separate
/// `REDUCE_UNRESOLVED-PROVENANCE` info finding naming which token failed
/// and why. The error finding stays the primary surface; the info finding
/// adds context without drowning the underlying error.
///
/// Returns `String::new()` for non-bridge relations (existing behavior
/// preserved — non-bridge `REDUCE_UNRESOLVED` findings stay unadorned).
///
/// F-002 (zfm audio-plugin dogfood, 2026-05-04): when a bridge binding
/// references a parameter or region on a different module than the
/// bindings table, the user sees a `REDUCE_UNRESOLVED` for the symptom
/// without the actionable advice (move the binding, or move the
/// declaration). This helper appends that advice from the bridge's
/// authored manifest — generic over every bridge, no per-bridge name in
/// the analyzer.
#[allow(clippy::too_many_arguments)]
fn render_bridge_hint(
    qualified_path: &str,
    unresolved_name: &str,
    opposite_kind: &str,
    opposite_name: &str,
    binding_module: Option<&str>,
    layout: &LayoutContext,
    entities: &[Entity],
    project_root: &Path,
    parent_provenance: &Provenance,
    cache: &mut std::collections::HashMap<String, Option<BridgeContract>>,
    report: &mut Report,
) -> String {
    let head = match qualified_path.split('.').next() {
        Some(h) => h,
        None => return String::new(),
    };
    if !head.contains('+') {
        return String::new();
    }

    // Cached load of `<reductions_dir>/bridges/<head>.contract.yaml`.
    // First-load surfaces malformed contracts as `MANIFEST-001` (Error)
    // to the report; subsequent cache hits skip the re-emit.
    let contract = if let Some(cached) = cache.get(head) {
        cached.clone()
    } else {
        let loaded = match load_bridge_contract(layout, head) {
            Ok(c) => c,
            Err(err) => {
                let path = layout
                    .reductions_dir
                    .join("bridges")
                    .join(format!("{head}.contract.yaml"));
                report.push(
                    Finding::new(
                        "MANIFEST-001",
                        Severity::Error,
                        format!("loading bridge contract.yaml failed: {err:#}"),
                    )
                    .with_file(path.display().to_string()),
                );
                None
            }
        };
        cache.insert(head.to_string(), loaded.clone());
        loaded
    };

    let Some(contract) = contract else {
        // Pre-manifest backward-compat: legacy generic hint string verbatim.
        return legacy_bridge_hint(head);
    };

    // Resolve placeholder values from the token table.
    //
    // - `{bridge_name}` is always available (qualified-path head).
    // - `{parameter}` is always available (the unresolved endpoint name
    //   on this side of the iteration).
    // - `{binding_module}` is the module owning the bindings table —
    //   the file the relation iteration is reading; mirrors the
    //   `parent_name` (file_root_name(&root_value)) computed earlier.
    // - `{decl_module}` is the OTHER endpoint's owning module. We look
    //   it up by searching the materialized graph for a matching
    //   `(opposite_kind, opposite_name)` entity and reading its
    //   provenance source file's `name` field. Fails when the opposite
    //   endpoint is also unresolved.
    let bridge_name = head.to_string();
    let parameter = unresolved_name.to_string();

    let mut unresolvable_tokens: Vec<(&'static str, String)> = Vec::new();

    let binding_module_resolved: String = match binding_module {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            unresolvable_tokens.push((
                "binding_module",
                "could not derive module name from the bindings file (file root has no `name` field)"
                    .to_string(),
            ));
            "<unknown>".to_string()
        }
    };

    let decl_module_resolved: String =
        match resolve_opposite_module(entities, opposite_kind, opposite_name, project_root) {
            Some(m) => m,
            None => {
                unresolvable_tokens.push((
                    "decl_module",
                    format!(
                        "opposite endpoint `{opposite_name}` ({opposite_kind}) does not resolve \
                         to a materialized entity — cannot read its owning module"
                    ),
                ));
                "<unknown>".to_string()
            }
        };

    let body = contract
        .diagnostic_template
        .trim_end_matches('\n')
        .replace("{bridge_name}", &bridge_name)
        .replace("{parameter}", &parameter)
        .replace("{decl_module}", &decl_module_resolved)
        .replace("{binding_module}", &binding_module_resolved);

    // Emit one info finding per unresolvable token, carrying
    // the same provenance as the parent error.
    for (token, reason) in &unresolvable_tokens {
        report.push(
            Finding::new(
                "REDUCE_UNRESOLVED-PROVENANCE",
                Severity::Info,
                format!("<{bridge_name}>: could not resolve {{{token}}} — {reason}"),
            )
            .with_provenance(parent_provenance.clone()),
        );
    }

    format!("\n  hint: {body}")
}

/// Pre-manifest backward-compat: the legacy hint text. Used verbatim
/// when a bridge has no inlined `contract.yaml`.
fn legacy_bridge_hint(head: &str) -> String {
    format!(
        "\n  hint: relation belongs to the `{head}` bridge — endpoint blocks must \
         co-reside on the SAME module that owns the bindings table; see \
         `bridges/{head}/README.md` for the v1 co-residency rule"
    )
}

/// Look up `<reductions_dir>/bridges/<head>.contract.yaml` and load it
/// via the strongly-typed loader. Three return shapes:
///   - `Ok(Some(contract))` — file present and valid.
///   - `Ok(None)` — file absent (pre-manifest backward-compat).
///   - `Err(_)` — file present but malformed; caller surfaces the
///     load error as `MANIFEST-001`.
fn load_bridge_contract(layout: &LayoutContext, head: &str) -> Result<Option<BridgeContract>> {
    let path = layout
        .reductions_dir
        .join("bridges")
        .join(format!("{head}.contract.yaml"));
    if !path.is_file() {
        return Ok(None);
    }
    BridgeContract::load(&path).map(Some)
}

/// Resolve the opposite endpoint's owning module by searching the
/// materialized graph for `(kind, name)` and reading its provenance
/// source file's `name` field. Returns `None` when:
///   - the opposite endpoint is also unresolved (no entity matches), or
///   - the source file cannot be read / parsed, or
///   - the file root has no `name` field.
fn resolve_opposite_module(
    entities: &[Entity],
    opposite_kind: &str,
    opposite_name: &str,
    project_root: &Path,
) -> Option<String> {
    let entity = entities
        .iter()
        .find(|e| e.kind == opposite_kind && e.name == opposite_name)?;
    let full = project_root.join(&entity.provenance.source_file);
    let root_value = load_toml_as_json(&full).ok()?;
    file_root_name(&root_value)
}

/// Translate the YAML body retained on a [`ResolvedEntityDecl`] /
/// [`ResolvedRelationDecl`] into a JSON map for selector evaluation. The
/// YAML carries the verbatim `from`/`name`/`attrs`/`endpoints` fields.
/// Materialize a `match:` relation: one edge from every already-built
/// entity of `from_kind` whose attribute is in the `from` value list to
/// every entity of `to_kind` whose attribute is in the `to` list. It
/// reads no spec file, so it has no TOML provenance; the provenance names
/// the reduction file and the relation kind instead, which keeps `reduce`
/// output deterministic. The shape was checked at resolve time
/// (`reductions::check_match_shape`), so a malformed body yields no edges
/// here without a second finding.
fn materialize_match_relation(
    qualified_path: &str,
    decl: &ResolvedRelationDecl,
    canonical: &str,
    m: &Map<String, Value>,
    project_root: &Path,
    graph: &mut Graph,
) {
    let from_kind = decl.from_kind.to_string();
    let to_kind = decl.to_kind.to_string();
    let (Some(from_f), Some(to_f)) = (
        m.get("from").and_then(MatchFilter::parse),
        m.get("to").and_then(MatchFilter::parse),
    ) else {
        return;
    };
    let pick = |kind: &str, f: &MatchFilter| -> Vec<String> {
        let mut names: Vec<String> = graph
            .entities
            .iter()
            .filter(|e| e.kind == kind && f.admits(e))
            .map(|e| e.name.clone())
            .collect();
        names.sort();
        names.dedup();
        names
    };
    let froms = pick(&from_kind, &from_f);
    let tos = pick(&to_kind, &to_f);
    let source_file = relpath(&decl.source_path, project_root);
    for f in &froms {
        for t in &tos {
            if f == t && from_kind == to_kind {
                continue;
            }
            graph.relations.push(Relation {
                kind: qualified_path.to_string(),
                extends: canonical.to_string(),
                from_kind: from_kind.clone(),
                to_kind: to_kind.clone(),
                from: f.clone(),
                to: t.clone(),
                attrs: BTreeMap::new(),
                provenance: Provenance::new(
                    source_file.clone(),
                    format!("/relations/{qualified_path}"),
                )
                .into(),
            });
        }
    }
}

/// One side of a `match:` relation: `{ attr: <name>, in: [<value>, …] }`.
struct MatchFilter {
    attr: String,
    values: Vec<String>,
}

impl MatchFilter {
    fn parse(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let attr = o.get("attr")?.as_str()?.to_string();
        let values = o
            .get("in")?
            .as_array()?
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect();
        Some(Self { attr, values })
    }

    fn admits(&self, e: &Entity) -> bool {
        e.attrs
            .get(&self.attr)
            .and_then(Value::as_str)
            .is_some_and(|a| self.values.iter().any(|v| v == a))
    }
}

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
    parent: Option<&'a Value>,
    pointer: String,
}

fn iterate<'a>(root: &'a Value, for_each: Option<&str>) -> Vec<Iter<'a>> {
    match for_each {
        None => vec![Iter {
            current: root,
            parent: None,
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
                    let parent = parent_value(root, &p.pointer);
                    Some(Iter {
                        current,
                        parent,
                        pointer: p.pointer,
                    })
                })
                .collect()
        }
    }
}

/// Closest enclosing object of the value at `pointer`. Walks the JSON
/// pointer's ancestors and returns the nearest one that is a JSON object
/// (skipping array-index segments). For a top-level pick `/foo/0` this
/// is the file root; for a nested pick `/items/0/sub/0` this is `items[0]`.
/// Returns `None` if `pointer` is empty (no parent context — the iteration
/// IS the root).
fn parent_value<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        return None;
    }
    let segments: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    if segments.is_empty() {
        return None;
    }
    let mut cur = root;
    let mut last_object: Option<&Value> = if root.is_object() { Some(root) } else { None };
    for seg in &segments[..segments.len() - 1] {
        cur = match cur {
            Value::Object(map) => match map.get(seg) {
                Some(v) => v,
                None => return last_object,
            },
            Value::Array(arr) => match seg.parse::<usize>().ok().and_then(|i| arr.get(i)) {
                Some(v) => v,
                None => return last_object,
            },
            _ => return last_object,
        };
        if cur.is_object() {
            last_object = Some(cur);
        }
    }
    last_object
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

pub(crate) fn load_toml_as_json(path: &Path) -> Result<Value> {
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
