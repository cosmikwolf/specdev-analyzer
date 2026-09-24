//! Declarative graph-check primitives.
//!
//! Each primitive consumes a JSON-like value and selector strings (see
//! [`crate::path`]), and returns zero or more [`Finding`]s. Scopes
//! (`module` / `per_state_machine` / `project`) are handled at the call
//! site — most primitives treat scope purely by which root value they
//! receive. For `per_state_machine` we group picks by their common
//! ancestor array element and run the primitive per-group.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use serde_json::Value;

use crate::domain::Severity;
use crate::path::{ancestor_at_array, select, Pick};
use crate::report::Finding;

fn value_as_str(v: &Value) -> Option<&str> {
    v.as_str()
}

fn group_by_ancestor(picks: Vec<Pick>, nth: usize) -> BTreeMap<String, Vec<Pick>> {
    let mut groups: BTreeMap<String, Vec<Pick>> = BTreeMap::new();
    for p in picks {
        let key = ancestor_at_array(&p.pointer, nth).unwrap_or_default();
        groups.entry(key).or_default().push(p);
    }
    groups
}

/// UNIQUE: every pick's scalar value must appear at most once.
///
/// When a pick's parent JSON object carries a sibling `kind` field
/// (typical of `entities[]` and `relations[]` shapes in the project
/// reduction), the error message names the colliding kinds. F-011
/// (2026-05-08 zfm dogfood): a generic `duplicate value` message
/// mis-frames cross-entity-kind collisions (e.g. an `egui.Region`
/// and a `nih-plug.Parameter` sharing a qualified name); naming the
/// kinds turns "what does this even mean" into a one-glance fix.
pub fn unique(root: &Value, field: &str, rule: &str, severity: Severity) -> Vec<Finding> {
    let picks = select(root, field);
    let mut seen: HashMap<String, SeenEntry> = HashMap::new();
    let mut out = Vec::new();
    for p in picks {
        let Some(s) = value_as_str(&p.value) else {
            continue;
        };
        let kind = sibling_kind(root, &p.pointer);
        if let Some(first) = seen.get(s) {
            out.push(
                Finding::new(
                    rule,
                    severity,
                    format_unique_message(s, kind.as_deref(), first),
                )
                .with_pointer(p.pointer.clone()),
            );
        } else {
            seen.insert(
                s.to_string(),
                SeenEntry {
                    pointer: p.pointer,
                    kind,
                },
            );
        }
    }
    out
}

#[derive(Debug)]
struct SeenEntry {
    pointer: String,
    kind: Option<String>,
}

fn format_unique_message(value: &str, this_kind: Option<&str>, first: &SeenEntry) -> String {
    let first_ptr = &first.pointer;
    match (this_kind, first.kind.as_deref()) {
        (Some(a), Some(b)) if a != b => format!(
            "duplicate value `{value}` — `{a}` collides with `{b}` \
             (first seen at {first_ptr}). Hint: rename one of the entries \
             or scope them under different parents."
        ),
        (Some(k), Some(_)) => format!(
            "duplicate value `{value}` — two `{k}` entries collide \
             (first seen at {first_ptr}). Hint: rename one of the entries."
        ),
        _ => format!("duplicate value `{value}` (first seen at {first_ptr})"),
    }
}

/// Look up the `kind` field on the JSON object that contains the picked
/// value. Given a pointer like `/entities/31/name`, the parent object is
/// at `/entities/31`; we read its `kind` if present and stringify it.
/// Returns `None` for pointers without a parent object segment, or when
/// the parent has no string `kind` field.
fn sibling_kind(root: &Value, pointer: &str) -> Option<String> {
    let parent = pointer.rsplit_once('/').map(|(p, _)| p)?;
    let parent_value = root.pointer(parent)?;
    parent_value
        .get("kind")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// REFERENCE_EXISTS: every source scalar must match some target scalar.
pub fn reference_exists(
    root: &Value,
    source_field: &str,
    target_field: &str,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let targets: BTreeSet<String> = select(root, target_field)
        .into_iter()
        .filter_map(|p| p.value.as_str().map(str::to_string))
        .collect();
    let mut out = Vec::new();
    for p in select(root, source_field) {
        let Some(s) = value_as_str(&p.value) else {
            continue;
        };
        if !targets.contains(s) {
            out.push(
                Finding::new(
                    rule,
                    severity,
                    format!("reference `{s}` not found in {target_field}"),
                )
                .with_pointer(p.pointer),
            );
        }
    }
    out
}

/// REFERENCE_EXISTS scoped per ancestor array index (e.g. per state_machine).
pub fn reference_exists_per(
    root: &Value,
    source_field: &str,
    target_field: &str,
    nth: usize,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let src_groups = group_by_ancestor(select(root, source_field), nth);
    let tgt_groups = group_by_ancestor(select(root, target_field), nth);
    let mut out = Vec::new();
    for (ancestor, sources) in src_groups {
        let targets: BTreeSet<String> = tgt_groups
            .get(&ancestor)
            .into_iter()
            .flat_map(|v| v.iter())
            .filter_map(|p| p.value.as_str().map(str::to_string))
            .collect();
        for p in sources {
            let Some(s) = value_as_str(&p.value) else {
                continue;
            };
            if !targets.contains(s) {
                out.push(
                    Finding::new(
                        rule,
                        severity,
                        format!("reference `{s}` not found in {target_field}"),
                    )
                    .with_pointer(p.pointer),
                );
            }
        }
    }
    out
}

/// REACHABILITY: every node must be reachable from entry via edges.
pub fn reachability(
    root: &Value,
    nodes_field: &str,
    edges_from: &str,
    edges_to: &str,
    entry_field: &str,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    reach_impl(
        root,
        nodes_field,
        edges_from,
        edges_to,
        entry_field,
        None,
        rule,
        severity,
    )
}

/// REACHABILITY scoped per ancestor array index.
#[allow(clippy::too_many_arguments)]
pub fn reachability_per(
    root: &Value,
    nodes_field: &str,
    edges_from: &str,
    edges_to: &str,
    entry_field: &str,
    nth: usize,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    reach_impl(
        root,
        nodes_field,
        edges_from,
        edges_to,
        entry_field,
        Some(nth),
        rule,
        severity,
    )
}

#[allow(clippy::too_many_arguments)]
fn reach_impl(
    root: &Value,
    nodes_field: &str,
    edges_from: &str,
    edges_to: &str,
    entry_field: &str,
    nth: Option<usize>,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let nodes = select(root, nodes_field);
    let froms = select(root, edges_from);
    let tos = select(root, edges_to);
    let entries = select(root, entry_field);
    let mut out = Vec::new();

    let groups: Vec<String> = match nth {
        None => vec![String::new()],
        Some(n) => {
            let mut s = BTreeSet::new();
            for p in &nodes {
                s.insert(ancestor_at_array(&p.pointer, n).unwrap_or_default());
            }
            s.into_iter().collect()
        }
    };

    for g in groups {
        let keep = |picks: &[Pick]| -> Vec<Pick> {
            match nth {
                None => picks.to_vec(),
                Some(n) => picks
                    .iter()
                    .filter(|p| ancestor_at_array(&p.pointer, n).as_deref() == Some(&g))
                    .cloned()
                    .collect(),
            }
        };
        let g_nodes = keep(&nodes);
        let g_froms = keep(&froms);
        let g_tos = keep(&tos);
        let g_entry = keep(&entries);

        let node_set: BTreeSet<String> = g_nodes
            .iter()
            .filter_map(|p| p.value.as_str().map(str::to_string))
            .collect();
        if node_set.is_empty() {
            continue;
        }
        let Some(entry) = g_entry
            .first()
            .and_then(|p| p.value.as_str())
            .map(str::to_string)
        else {
            continue;
        };

        let pairs: Vec<(String, String)> = g_froms
            .iter()
            .zip(g_tos.iter())
            .filter_map(|(a, b)| {
                let f = a.value.as_str()?.to_string();
                let t = b.value.as_str()?.to_string();
                Some((f, t))
            })
            .collect();
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for (f, t) in &pairs {
            adj.entry(f.clone()).or_default().push(t.clone());
        }

        let mut visited: BTreeSet<String> = BTreeSet::new();
        let mut q: VecDeque<String> = VecDeque::new();
        q.push_back(entry.clone());
        visited.insert(entry);
        while let Some(n) = q.pop_front() {
            if let Some(next) = adj.get(&n) {
                for v in next {
                    if visited.insert(v.clone()) {
                        q.push_back(v.clone());
                    }
                }
            }
        }

        for p in &g_nodes {
            let Some(name) = p.value.as_str() else {
                continue;
            };
            if !visited.contains(name) {
                out.push(
                    Finding::new(
                        rule,
                        severity,
                        format!("node `{name}` unreachable from entry"),
                    )
                    .with_pointer(p.pointer.clone()),
                );
            }
        }
    }
    out
}

/// REQUIREMENTS_HAVE_SATISFIERS (F-006): each entity in scope (typically
/// Requirement) with `attrs.nature` ∈ {`"module"`, missing} must have at
/// least one incoming relation in the relation scope (typically
/// `module_satisfies`). Entities with `nature` set to `"architecture"`,
/// `"build"`, or `"test"` are exempt — they intentionally don't have a
/// module satisfier.
///
/// The caller is expected to pre-filter the JSON view via
/// `filter_for_layer_dependency`-style joint filtering so that
/// `entities[]` contains only the entity scope and `relations[]` only
/// the relation scope. The primitive itself does no kind filtering.
///
/// Emits one finding per orphan entity, naming the requirement id and
/// the recognised nature values for the user's reference.
pub fn requirements_have_satisfiers(root: &Value, rule: &str, severity: Severity) -> Vec<Finding> {
    let entities = root.get("entities").and_then(|v| v.as_array());
    let relations = root.get("relations").and_then(|v| v.as_array());
    let Some(entities) = entities else {
        return Vec::new();
    };
    let satisfied: BTreeSet<String> = relations
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.get("to").and_then(|t| t.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    for ent in entities {
        let Some(name) = ent.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let nature = ent
            .get("attrs")
            .and_then(|a| a.get("nature"))
            .and_then(|n| n.as_str())
            .unwrap_or("module");
        if nature != "module" {
            continue;
        }
        if satisfied.contains(name) {
            continue;
        }
        out.push(Finding::new(
            rule,
            severity,
            format!(
                "requirement `{name}` (nature = \"module\") has no module satisfier; \
                 add it to a module's `satisfies = [...]` array, or set `nature = \
                 \"architecture\" | \"build\" | \"test\"` to exempt"
            ),
        ));
    }
    out
}

/// ENTITY_HAS_INBOUND_RELATION: each entity in scope must be the target of
/// at least one relation in scope. The generic orphan check — "every X is
/// referenced by at least one Y." Unlike `requirements_have_satisfiers`
/// (which carries requirement-specific vocabulary and a `nature`-based
/// exemption), this primitive is vocabulary-neutral: it names the entity's
/// own `kind` and the relation label in the diagnostic, so it reads
/// correctly for any (entity, relation) pairing a profile registers.
///
/// The caller pre-filters the JSON view via `filter_for_layer_dependency`
/// so that `entities[]` holds only the entity scope and `relations[]` only
/// the relation scope; the primitive itself does no kind filtering. The
/// `relation_label` is the `relation_scope` string from the check config,
/// used verbatim in the message.
///
/// Optional attr filter: when `entity_attr` + `entity_attr_value` are
/// both `Some`, only entities whose `attrs.<entity_attr>` equals the value
/// are checked (others are exempt). This is how
/// `test_requirements_have_verifiers` scopes the orphan check to
/// `nature = "test"` requirements without disturbing the unfiltered
/// `nature`-agnostic consumers (audio-plugin's `parameters_have_bindings`
/// passes `None`/`None`). A missing attr never matches a concrete filter
/// value, so attr-filtered checks silently skip entities that don't carry
/// the attr at all.
///
/// Emits one finding per orphan entity.
pub fn entity_has_inbound_relation(
    root: &Value,
    relation_label: &str,
    entity_attr: Option<&str>,
    entity_attr_value: Option<&str>,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let Some(entities) = root.get("entities").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let referenced: BTreeSet<String> = root
        .get("relations")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| r.get("to").and_then(|t| t.as_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut out = Vec::new();
    for ent in entities {
        let Some(name) = ent.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        // Optional attr filter: skip entities that don't match the
        // requested `attrs.<attr> == value`. Missing attr never matches.
        if let (Some(attr), Some(want)) = (entity_attr, entity_attr_value) {
            let have = ent
                .get("attrs")
                .and_then(|a| a.get(attr))
                .and_then(|v| v.as_str());
            if have != Some(want) {
                continue;
            }
        }
        if referenced.contains(name) {
            continue;
        }
        // Human-readable noun: the last path segment of the entity kind
        // (e.g. `nih-plug.Parameter` → `Parameter`). Falls back to the
        // whole kind, then to "entity".
        let noun = ent
            .get("kind")
            .and_then(|k| k.as_str())
            .map(|k| k.rsplit('.').next().unwrap_or(k))
            .unwrap_or("entity");
        out.push(Finding::new(
            rule,
            severity,
            format!(
                "{noun} `{name}` has no inbound `{relation_label}` relation; \
                 every {noun} must be referenced by at least one `{relation_label}`"
            ),
        ));
    }
    out
}

/// ACYCLIC: the directed graph defined by (edges_from, edges_to) has no cycles.
pub fn acyclic(
    root: &Value,
    edges_from: &str,
    edges_to: &str,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    // Group both endpoints by their containing array element (e.g. for
    // `modules[].name` and `modules[].depends_on[]`, the top-level `modules[]`
    // ancestor). This pairs each source with the dependency array that lives
    // on the same module, instead of zipping the two flat lists in
    // declaration order.
    let froms = select(root, edges_from);
    let tos = select(root, edges_to);
    let mut from_by_anc: HashMap<String, String> = HashMap::new();
    for p in &froms {
        if let Some(anc) = ancestor_at_array(&p.pointer, 0) {
            if let Some(s) = p.value.as_str() {
                from_by_anc.insert(anc, s.to_string());
            }
        }
    }
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for p in &tos {
        let Some(anc) = ancestor_at_array(&p.pointer, 0) else {
            continue;
        };
        let Some(from) = from_by_anc.get(&anc) else {
            continue;
        };
        let Some(t) = p.value.as_str() else { continue };
        adj.entry(from.clone()).or_default().push(t.to_string());
    }
    // Kahn's algorithm on a distinct-node subgraph derived from edges.
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    for (f, ts) in &adj {
        nodes.insert(f.clone());
        for t in ts {
            nodes.insert(t.clone());
        }
    }
    let mut indeg: HashMap<String, usize> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    for ts in adj.values() {
        for t in ts {
            *indeg.entry(t.clone()).or_insert(0) += 1;
        }
    }
    let mut q: VecDeque<String> = indeg
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut removed = 0usize;
    while let Some(n) = q.pop_front() {
        removed += 1;
        if let Some(next) = adj.get(&n) {
            for v in next {
                if let Some(e) = indeg.get_mut(v) {
                    *e -= 1;
                    if *e == 0 {
                        q.push_back(v.clone());
                    }
                }
            }
        }
    }
    if removed < nodes.len() {
        let cycle: Vec<String> = indeg
            .iter()
            .filter(|(_, &d)| d > 0)
            .map(|(n, _)| n.clone())
            .collect();
        vec![Finding::new(
            rule,
            severity,
            format!("cycle detected among nodes: {}", cycle.join(", ")),
        )]
    } else {
        Vec::new()
    }
}

/// ENTITY_ATTR_INBOUND_COUNT: for every entity whose `attrs.<entity_attr>`
/// equals `entity_attr_value`, count inbound relations (over the
/// `relation_scope`-filtered relations[]) and assert the count equals
/// `expected_count`. Used by `decision_supersedes_chain_consistency` to
/// guarantee each superseded Decision is claimed by exactly one inbound
/// `decision_supersedes_decision` edge.
///
/// The caller is expected to pre-filter `relations[]` jointly via
/// `filter_for_layer_dependency`-style filtering so this primitive only
/// sees the relation kind under consideration. Entities are filtered
/// inside the primitive via `entity_attr` + `entity_attr_value`.
pub fn entity_attr_inbound_count(
    root: &Value,
    entity_attr: &str,
    entity_attr_value: &str,
    expected_count: usize,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let entities = root.get("entities").and_then(|v| v.as_array());
    let relations = root.get("relations").and_then(|v| v.as_array());
    let Some(entities) = entities else {
        return Vec::new();
    };

    // Count inbound edges per target name.
    let mut inbound: HashMap<String, usize> = HashMap::new();
    if let Some(rels) = relations {
        for r in rels {
            if let Some(to) = r.get("to").and_then(|t| t.as_str()) {
                *inbound.entry(to.to_string()).or_insert(0) += 1;
            }
        }
    }

    let mut out = Vec::new();
    for ent in entities {
        let Some(name) = ent.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let attr_value = ent
            .get("attrs")
            .and_then(|a| a.get(entity_attr))
            .and_then(|v| v.as_str());
        if attr_value != Some(entity_attr_value) {
            continue;
        }
        let count = inbound.get(name).copied().unwrap_or(0);
        if count != expected_count {
            out.push(Finding::new(
                rule,
                severity,
                format!(
                    "entity `{name}` (with `{entity_attr} = \"{entity_attr_value}\"`) \
                     has {count} inbound relation(s); expected {expected_count}"
                ),
            ));
        }
    }
    out
}

/// ENTITY_COUNT: count the entities (already filtered to the check's
/// `entity_scope`) whose `entity_attr` equals `entity_attr_value`, and
/// assert the count equals `expected_count`. An empty scope passes: a
/// project with no entity of the kind yet (a fresh bootstrap) has
/// nothing to count, and a finding there would be noise.
pub fn entity_count(
    root: &Value,
    entity_scope: &str,
    entity_attr: &str,
    entity_attr_value: &str,
    expected_count: usize,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let Some(entities) = root.get("entities").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    if entities.is_empty() {
        return Vec::new();
    }
    let mut names: Vec<&str> = entities
        .iter()
        .filter(|e| {
            e.get("attrs")
                .and_then(|a| a.get(entity_attr))
                .and_then(|v| v.as_str())
                == Some(entity_attr_value)
        })
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .collect();
    names.sort_unstable();
    if names.len() == expected_count {
        return Vec::new();
    }
    let listed = if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    };
    vec![Finding::new(
        rule,
        severity,
        format!(
            "{} `{entity_scope}` entities have `{entity_attr} = \"{entity_attr_value}\"` \
             ({listed}); expected {expected_count}",
            names.len()
        ),
    )]
}

/// ENTITY_ATTR_TARGET_ATTR_EQUALS: for every entity matching the source
/// filter (`entity_attr == entity_attr_value`), follow the source entity's
/// `target_attr_field` attribute to a target entity (must exist, must be
/// of kind `target_entity_kind`), then verify the target's `target_attr`
/// equals `target_attr_value`. Used by `open_question_status_consistency`
/// to enforce that a closed OQ's `closed_by` Decision is `accepted` (and
/// likewise for default_committed → default_via).
///
/// Operates on the unfiltered graph view so it can reach target entities
/// of a kind the source filter would exclude.
#[allow(clippy::too_many_arguments)]
pub fn entity_attr_target_attr_equals(
    root: &Value,
    entity_kind: &str,
    entity_attr: &str,
    entity_attr_value: &str,
    target_attr_field: &str,
    target_entity_kind: &str,
    target_attr: &str,
    target_attr_value: &str,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let entities = root.get("entities").and_then(|v| v.as_array());
    let Some(entities) = entities else {
        return Vec::new();
    };

    // Build (name → entity) for cross-resolution.
    let mut by_name: BTreeMap<String, &Value> = BTreeMap::new();
    for ent in entities {
        if let Some(n) = ent.get("name").and_then(|v| v.as_str()) {
            by_name.insert(n.to_string(), ent);
        }
    }

    let mut out = Vec::new();
    for ent in entities {
        let Some(kind) = ent.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        if kind != entity_kind {
            continue;
        }
        let Some(name) = ent.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let attr_value = ent
            .get("attrs")
            .and_then(|a| a.get(entity_attr))
            .and_then(|v| v.as_str());
        if attr_value != Some(entity_attr_value) {
            continue;
        }
        let Some(target_name) = ent
            .get("attrs")
            .and_then(|a| a.get(target_attr_field))
            .and_then(|v| v.as_str())
        else {
            out.push(Finding::new(
                rule,
                severity,
                format!(
                    "entity `{name}` (with `{entity_attr} = \"{entity_attr_value}\"`) \
                     is missing required attr `{target_attr_field}` — cannot verify \
                     target `{target_attr} = \"{target_attr_value}\"`"
                ),
            ));
            continue;
        };
        let Some(target) = by_name.get(target_name) else {
            // reference_exists already flags this for relation-shaped
            // refs; for attr-shaped refs we rely on the dedicated
            // `*_closed_by_resolves` check. Don't double-report here.
            continue;
        };
        let target_kind = target.get("kind").and_then(|k| k.as_str()).unwrap_or("");
        if target_kind != target_entity_kind {
            out.push(Finding::new(
                rule,
                severity,
                format!(
                    "entity `{name}`'s `{target_attr_field}` points at `{target_name}` \
                     of kind `{target_kind}`; expected kind `{target_entity_kind}`"
                ),
            ));
            continue;
        }
        let actual = target
            .get("attrs")
            .and_then(|a| a.get(target_attr))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if actual != target_attr_value {
            out.push(Finding::new(
                rule,
                severity,
                format!(
                    "entity `{name}` (with `{entity_attr} = \"{entity_attr_value}\"`) \
                     references `{target_name}` whose `{target_attr}` is `\"{actual}\"`; \
                     expected `\"{target_attr_value}\"`"
                ),
            ));
        }
    }
    out
}

/// MODULE_REACHABLE_THROUGH_OPEN_QUESTION (codegen-gate informational
/// warning): for each Module entity, walk Module.satisfies →
/// Requirement → OpenQuestion (via open_question_affects_requirement
/// reversed). If any reachable OQ has `attrs.status == "open"`, fire one
/// finding per (module, oq) pair. The companion gate in `gen.rs` produces
/// the hard refusal; this primitive surfaces the same condition at
/// validate time as a warning.
///
/// Operates on the unfiltered graph view because it traverses two
/// relation kinds and two entity kinds; the call site doesn't pre-filter.
pub fn module_reachable_through_open_question(
    root: &Value,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let entities = root
        .get("entities")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let relations = root
        .get("relations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Index entities by name for kind + attr lookup.
    let mut by_name: HashMap<String, Value> = HashMap::new();
    for ent in &entities {
        if let Some(n) = ent.get("name").and_then(|v| v.as_str()) {
            by_name.insert(n.to_string(), ent.clone());
        }
    }

    // module_satisfies edges: Module → Requirement.
    let mut module_to_reqs: HashMap<String, Vec<String>> = HashMap::new();
    // affects edges (reversed): Requirement → OpenQuestion.
    let mut req_to_oqs: HashMap<String, Vec<String>> = HashMap::new();
    for rel in &relations {
        let Some(kind) = rel.get("kind").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(from) = rel.get("from").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(to) = rel.get("to").and_then(|v| v.as_str()) else {
            continue;
        };
        match kind {
            "module_satisfies" => {
                module_to_reqs
                    .entry(from.to_string())
                    .or_default()
                    .push(to.to_string());
            }
            "open_question_affects_requirement" => {
                // Edge is OQ → Requirement; we want Requirement → OQ.
                req_to_oqs
                    .entry(to.to_string())
                    .or_default()
                    .push(from.to_string());
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    let mut module_names: Vec<&String> = by_name
        .iter()
        .filter_map(|(n, e)| {
            let kind = e.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            (kind == "Module").then_some(n)
        })
        .collect();
    module_names.sort();

    for module in module_names {
        let mut emitted: BTreeSet<String> = BTreeSet::new();
        let Some(reqs) = module_to_reqs.get(module) else {
            continue;
        };
        for req in reqs {
            let Some(oqs) = req_to_oqs.get(req) else {
                continue;
            };
            for oq in oqs {
                if emitted.contains(oq) {
                    continue;
                }
                let Some(oq_ent) = by_name.get(oq) else {
                    continue;
                };
                let status = oq_ent
                    .get("attrs")
                    .and_then(|a| a.get("status"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if status == "open" {
                    let title = oq_ent
                        .get("attrs")
                        .and_then(|a| a.get("title"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    out.push(Finding::new(
                        rule,
                        severity,
                        format!(
                            "module `{module}` is reachable through open question \
                             `{oq}` (`{title}`) via requirement `{req}`; codegen will \
                             refuse to emit until the question is resolved or a default \
                             is committed"
                        ),
                    ));
                    emitted.insert(oq.clone());
                }
            }
        }
    }
    out
}

/// LAYER_DEPENDENCY (graph-shape): for each relation in the
/// pre-filtered relation set, both endpoints must resolve to entities in
/// the pre-filtered entity set, and the `from` entity's layer must appear
/// in `ordering` at a position less than or equal to the `to` entity's
/// position. Same-layer dependencies are allowed; "upward" dependencies
/// (from a higher layer to a lower one in the ordering) are flagged.
///
/// Caller responsibilities:
/// - Pre-filter `root` so that `entities[]` contains only the kinds that
///   carry layers (e.g. Module-extending), and `relations[]` contains only
///   the relation kind being checked (e.g. `depends_on`-extending). The
///   filter pass lives in `validate.rs::filter_for_layer_dependency`.
/// - Pass the path to the layer attribute on each entity (e.g.
///   `"attrs.layer"`).
/// - Pass the layer ordering top-down (e.g.
///   `["application", "middleware", "driver", "HAL", "PAC"]`).
pub fn layer_dependency_graph(
    root: &Value,
    layer_field: &str,
    ordering: &[String],
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let layer_by_name = entity_layers(root, layer_field);
    let mut out = Vec::new();
    for edge in relation_edges(root) {
        let Some(from_layer) = layer_by_name.get(&edge.from) else {
            continue;
        };
        let Some(to_layer) = layer_by_name.get(&edge.to) else {
            continue;
        };
        match edge_direction(ordering, from_layer, to_layer) {
            EdgeDirection::UnknownFrom => out.push(Finding::new(
                rule,
                severity,
                format!(
                    "module `{}` declares unknown layer `{from_layer}` (not in ordering)",
                    edge.from
                ),
            )),
            EdgeDirection::UnknownTo => out.push(Finding::new(
                rule,
                severity,
                format!(
                    "module `{}` declares unknown layer `{to_layer}` (not in ordering)",
                    edge.to
                ),
            )),
            EdgeDirection::Upward => out.push(
                Finding::new(
                    rule,
                    severity,
                    format!(
                        "module `{}` (layer {from_layer}) depends on `{}` (layer {to_layer}); upward dependency violates ordering {ordering:?}",
                        edge.from, edge.to
                    ),
                )
                .with_pointer(edge.pointer),
            ),
            EdgeDirection::Allowed => {}
        }
    }
    out
}

/// One `relations[]` entry of a pre-filtered graph view: its endpoints by
/// name and the JSON pointer of its `to` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationEdge {
    pub from: String,
    pub to: String,
    pub pointer: String,
}

/// How one edge sits against a top-down layer ordering. Shared by the
/// `layer_dependency` check and `report graph`, so the report marks
/// exactly the edges the check flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    /// Same layer or downward.
    Allowed,
    /// From a lower layer to a higher one: the check's violation.
    Upward,
    /// The `from` entity's layer is not in the ordering.
    UnknownFrom,
    /// The `to` entity's layer is not in the ordering.
    UnknownTo,
}

/// Top-down ordering: lower index = higher layer. `from` may depend on
/// the same or lower layers (= same-or-higher index).
pub fn edge_direction(ordering: &[String], from_layer: &str, to_layer: &str) -> EdgeDirection {
    let pos = |l: &str| ordering.iter().position(|o| o == l);
    match (pos(from_layer), pos(to_layer)) {
        (None, _) => EdgeDirection::UnknownFrom,
        (_, None) => EdgeDirection::UnknownTo,
        (Some(f), Some(t)) if f > t => EdgeDirection::Upward,
        _ => EdgeDirection::Allowed,
    }
}

/// `name → layer` over the (already filtered) `entities[]`, reading the
/// layer at `layer_field` (e.g. `"attrs.layer"`). Entities without a
/// string at that path are absent from the map.
pub fn entity_layers(root: &Value, layer_field: &str) -> HashMap<String, String> {
    let mut layer_by_name: HashMap<String, String> = HashMap::new();
    let layer_lookup_path = format!("entities[].{layer_field}");
    let name_picks = select(root, "entities[].name");
    let layer_picks = select(root, &layer_lookup_path);
    let mut layer_by_anc: HashMap<String, String> = HashMap::new();
    for p in &layer_picks {
        if let Some(anc) = ancestor_at_array(&p.pointer, 0) {
            if let Some(s) = p.value.as_str() {
                layer_by_anc.insert(anc, s.to_string());
            }
        }
    }
    for p in &name_picks {
        if let Some(anc) = ancestor_at_array(&p.pointer, 0) {
            if let Some(nm) = p.value.as_str() {
                if let Some(layer) = layer_by_anc.get(&anc) {
                    layer_by_name.insert(nm.to_string(), layer.clone());
                }
            }
        }
    }
    layer_by_name
}

/// Every `relations[]` entry with a string `from` and `to`, paired by
/// array ancestor, in `relations[]` order.
pub fn relation_edges(root: &Value) -> Vec<RelationEdge> {
    let froms = select(root, "relations[].from");
    let tos = select(root, "relations[].to");
    let mut from_by_anc: HashMap<String, String> = HashMap::new();
    for p in &froms {
        if let Some(anc) = ancestor_at_array(&p.pointer, 0) {
            if let Some(s) = p.value.as_str() {
                from_by_anc.insert(anc, s.to_string());
            }
        }
    }
    let mut out = Vec::new();
    for p in tos {
        let Some(anc) = ancestor_at_array(&p.pointer, 0) else {
            continue;
        };
        let Some(from) = from_by_anc.get(&anc) else {
            continue;
        };
        let Some(to) = p.value.as_str() else {
            continue;
        };
        out.push(RelationEdge {
            from: from.clone(),
            to: to.to_string(),
            pointer: p.pointer,
        });
    }
    out
}

/// LAYER_DEPENDENCY (legacy source-data shape): retained for the existing
/// unit test and any caller that still passes raw `modules[].layer` /
/// `modules[].depends_on[]` selectors. New callers use
/// [`layer_dependency_graph`] above.
pub fn layer_dependency(
    root: &Value,
    module_layer_field: &str,
    module_deps_field: &str,
    allowed: &BTreeMap<String, Vec<String>>,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    // Build module → (layer, pointer) and module → [deps].
    // We group by `modules[i]` (ancestor at array 0 of layer picks and
    // name picks). The name picks come from the sibling "name" field.
    // To avoid needing the name field to be passed separately, we derive it
    // from the module_layer_field by swapping the last segment.
    let name_field = swap_last(module_layer_field, "name");
    let names = select(root, &name_field);
    let layers = select(root, module_layer_field);
    let deps = select(root, module_deps_field);

    let mut name_by_anc: HashMap<String, String> = HashMap::new();
    for p in names {
        if let Some(anc) = ancestor_at_array(&p.pointer, 0) {
            if let Some(s) = p.value.as_str() {
                name_by_anc.insert(anc, s.to_string());
            }
        }
    }
    let mut layer_by_name: HashMap<String, String> = HashMap::new();
    let mut layer_ptr_by_name: HashMap<String, String> = HashMap::new();
    for p in layers {
        let Some(anc) = ancestor_at_array(&p.pointer, 0) else {
            continue;
        };
        let Some(nm) = name_by_anc.get(&anc) else {
            continue;
        };
        if let Some(s) = p.value.as_str() {
            layer_by_name.insert(nm.clone(), s.to_string());
            layer_ptr_by_name.insert(nm.clone(), p.pointer.clone());
        }
    }

    let mut out = Vec::new();
    for p in deps {
        let Some(anc) = ancestor_at_array(&p.pointer, 0) else {
            continue;
        };
        let Some(owner) = name_by_anc.get(&anc) else {
            continue;
        };
        let Some(owner_layer) = layer_by_name.get(owner) else {
            continue;
        };
        let Some(dep_name) = p.value.as_str() else {
            continue;
        };
        let Some(dep_layer) = layer_by_name.get(dep_name) else {
            // handled by reference_exists elsewhere
            continue;
        };
        let empty: Vec<String> = Vec::new();
        let allow = allowed.get(owner_layer).unwrap_or(&empty);
        if !allow.iter().any(|l| l == dep_layer) {
            out.push(
                Finding::new(
                    rule,
                    severity,
                    format!(
                        "module `{owner}` (layer {owner_layer}) depends on `{dep_name}` (layer {dep_layer}), not in allowed_deps {allow:?}"
                    ),
                )
                .with_pointer(p.pointer),
            );
        }
    }
    out
}

/// Replace the segment after the last `.` with `replacement`.
/// e.g. swap_last("modules[].layer", "name") -> "modules[].name".
fn swap_last(field: &str, replacement: &str) -> String {
    match field.rfind('.') {
        Some(i) => format!("{}.{}", &field[..i], replacement),
        None => replacement.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unique_detects_dup() {
        let v = json!({ "xs": [{ "n": "a" }, { "n": "a" }] });
        let f = unique(&v, "xs[].n", "U1", Severity::Error);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn unique_message_names_colliding_kinds_when_different() {
        // F-011: two `entities[]` rows sharing a qualified name but
        // declaring different `kind`s — the message must surface both
        // kinds so a TOML author can find the offending blocks.
        let v = json!({
            "entities": [
                { "kind": "nih-plug.Parameter", "name": "shell.algorithm" },
                { "kind": "egui.Region",        "name": "shell.algorithm" },
            ],
        });
        let f = unique(
            &v,
            "entities[].name",
            "unique_qualified_names",
            Severity::Error,
        );
        assert_eq!(f.len(), 1);
        let msg = &f[0].message;
        assert!(msg.contains("shell.algorithm"), "missing value: {msg}");
        assert!(
            msg.contains("`nih-plug.Parameter`"),
            "missing kind A: {msg}"
        );
        assert!(msg.contains("`egui.Region`"), "missing kind B: {msg}");
        assert!(
            msg.contains("collides"),
            "expected collision phrasing: {msg}"
        );
    }

    #[test]
    fn unique_message_names_kind_when_same_kind_collides() {
        // Same-kind collision (legitimate "two of these" case) — the
        // message names the kind once and asks for a rename.
        let v = json!({
            "entities": [
                { "kind": "Module", "name": "audio" },
                { "kind": "Module", "name": "audio" },
            ],
        });
        let f = unique(
            &v,
            "entities[].name",
            "unique_qualified_names",
            Severity::Error,
        );
        assert_eq!(f.len(), 1);
        let msg = &f[0].message;
        assert!(msg.contains("`Module`"), "missing kind in message: {msg}");
    }

    #[test]
    fn unique_message_falls_back_when_no_kind_sibling() {
        // Generic UNIQUE use (not against `entities[]`) — picked nodes
        // have no `kind` sibling; message degrades to the legacy form.
        let v = json!({ "xs": [{ "n": "a" }, { "n": "a" }] });
        let f = unique(&v, "xs[].n", "U1", Severity::Error);
        assert_eq!(f.len(), 1);
        let msg = &f[0].message;
        assert!(msg.contains("duplicate value `a`"), "{msg}");
        assert!(
            !msg.contains("collides"),
            "no kind info should yield no collide phrasing: {msg}"
        );
    }

    #[test]
    fn reference_exists_flags_missing() {
        let v = json!({ "src": [{ "x": "b" }], "tgt": [{ "y": "a" }] });
        let f = reference_exists(&v, "src[].x", "tgt[].y", "R1", Severity::Error);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn entity_has_inbound_relation_flags_orphan_with_neutral_noun() {
        // Two Parameters; only one is a binding target. The other is an
        // orphan and the message must use the entity's own kind noun
        // (last path segment), not requirement vocabulary.
        let v = json!({
            "entities": [
                { "kind": "nih-plug.Parameter", "name": "synth.op1_crs" },
                { "kind": "nih-plug.Parameter", "name": "synth.master_volume" },
            ],
            "relations": [
                { "kind": "parameter_binding", "from": "synth", "to": "synth.op1_crs" },
            ],
        });
        let f = entity_has_inbound_relation(
            &v,
            "parameter_binding",
            None,
            None,
            "parameters_have_bindings",
            Severity::Warning,
        );
        assert_eq!(f.len(), 1, "exactly one orphan expected");
        let msg = &f[0].message;
        assert!(
            msg.contains("synth.master_volume"),
            "names the orphan: {msg}"
        );
        assert!(
            msg.contains("Parameter "),
            "uses last-segment noun, not the qualified kind: {msg}"
        );
        assert!(
            msg.contains("`parameter_binding`"),
            "names the relation: {msg}"
        );
        assert!(
            !msg.contains("requirement"),
            "must not leak requirement vocabulary: {msg}"
        );
    }

    #[test]
    fn entity_has_inbound_relation_multi_target_param_is_satisfied() {
        // A parameter may carry multiple bindings. Two edges to the
        // same Parameter still leave zero orphans.
        let v = json!({
            "entities": [{ "kind": "nih-plug.Parameter", "name": "synth.algorithm" }],
            "relations": [
                { "kind": "parameter_binding", "from": "synth", "to": "synth.algorithm" },
                { "kind": "parameter_binding", "from": "synth", "to": "synth.algorithm" },
            ],
        });
        let f = entity_has_inbound_relation(
            &v,
            "parameter_binding",
            None,
            None,
            "parameters_have_bindings",
            Severity::Warning,
        );
        assert!(
            f.is_empty(),
            "multi-bound parameter is not an orphan: {f:?}"
        );
    }

    #[test]
    fn entity_has_inbound_relation_attr_filter_scopes_to_matching_entities() {
        // Only `nature = "test"` requirements are checked for an
        // inbound test_verifies edge. A nature=module requirement with no
        // test_verifies must NOT be flagged (it's out of scope), and a
        // nature=test requirement with no verifier MUST be flagged.
        let v = json!({
            "entities": [
                { "kind": "Requirement", "name": "REQ-1", "attrs": { "nature": "module" } },
                { "kind": "Requirement", "name": "REQ-2", "attrs": { "nature": "test" } },
                { "kind": "Requirement", "name": "REQ-3", "attrs": { "nature": "test" } },
            ],
            "relations": [
                { "kind": "test_verifies", "from": "m", "to": "REQ-2" },
            ],
        });
        let f = entity_has_inbound_relation(
            &v,
            "test_verifies",
            Some("nature"),
            Some("test"),
            "test_requirements_have_verifiers",
            Severity::Warning,
        );
        assert_eq!(
            f.len(),
            1,
            "only the unverified test requirement is flagged: {f:?}"
        );
        assert!(
            f[0].message.contains("REQ-3"),
            "names REQ-3: {}",
            f[0].message
        );
        assert!(
            !f[0].message.contains("REQ-1"),
            "must not flag the module req"
        );
    }

    #[test]
    fn reachability_detects_unreachable_state() {
        let v = json!({
            "sms": [{
                "initial": "s0",
                "states": ["s0", "s1", "s2"],
                "transitions": [{ "from": "s0", "to": "s1" }],
            }],
        });
        let f = reachability_per(
            &v,
            "sms[].states[]",
            "sms[].transitions[].from",
            "sms[].transitions[].to",
            "sms[].initial",
            0,
            "F22",
            Severity::Warning,
        );
        assert_eq!(f.len(), 1);
        assert!(f[0].message.contains("s2"));
    }

    #[test]
    fn acyclic_detects_cycle() {
        let v = json!({
            "modules": [
                { "name": "A", "depends_on": ["B"] },
                { "name": "B", "depends_on": ["A"] },
            ],
        });
        let f = acyclic(
            &v,
            "modules[].name",
            "modules[].depends_on[]",
            "L2",
            Severity::Error,
        );
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn layer_dependency_graph_flags_upward_edge() {
        // Module A is at layer "driver"; module B is at layer "application".
        // A → B is upward (driver depends on application) which is a violation.
        let v = json!({
            "entities": [
                { "kind": "Module", "name": "A", "attrs": { "layer": "driver" } },
                { "kind": "Module", "name": "B", "attrs": { "layer": "application" } },
            ],
            "relations": [
                { "kind": "depends_on", "from": "A", "to": "B" },
            ],
        });
        let ordering = vec![
            "application".to_string(),
            "middleware".to_string(),
            "driver".to_string(),
            "HAL".to_string(),
            "PAC".to_string(),
        ];
        let f = layer_dependency_graph(&v, "attrs.layer", &ordering, "L1", Severity::Error);
        assert_eq!(f.len(), 1, "{f:?}");
    }

    #[test]
    fn layer_dependency_graph_allows_same_and_downward() {
        // application -> middleware (downward) and middleware -> middleware (same)
        // should both pass.
        let v = json!({
            "entities": [
                { "kind": "Module", "name": "App", "attrs": { "layer": "application" } },
                { "kind": "Module", "name": "Mid1", "attrs": { "layer": "middleware" } },
                { "kind": "Module", "name": "Mid2", "attrs": { "layer": "middleware" } },
            ],
            "relations": [
                { "kind": "depends_on", "from": "App", "to": "Mid1" },
                { "kind": "depends_on", "from": "Mid1", "to": "Mid2" },
            ],
        });
        let ordering = vec!["application".into(), "middleware".into(), "driver".into()];
        let f = layer_dependency_graph(&v, "attrs.layer", &ordering, "L1", Severity::Error);
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn layer_dependency_flags_violation() {
        let v = json!({
            "modules": [
                { "name": "M1", "layer": "driver", "depends_on": ["M2"] },
                { "name": "M2", "layer": "middleware", "depends_on": [] },
            ],
        });
        let mut allowed: BTreeMap<String, Vec<String>> = BTreeMap::new();
        allowed.insert("driver".into(), vec!["HAL".into()]);
        allowed.insert("middleware".into(), vec!["driver".into()]);
        let f = layer_dependency(
            &v,
            "modules[].layer",
            "modules[].depends_on[]",
            &allowed,
            "L1",
            Severity::Error,
        );
        assert_eq!(f.len(), 1);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// Property: `unique` reports exactly N - distinct(N) findings
        /// for an array of N name strings. The unit test covers the
        /// 2-of-2-duplicates case; this widens to arbitrary sizes and
        /// patterns including all-distinct and all-same.
        #[test]
        fn unique_reports_dup_count(
            names in proptest::collection::vec("[a-z]{1,4}", 0..30)
        ) {
            let items: Vec<serde_json::Value> = names
                .iter()
                .map(|n| json!({ "name": n }))
                .collect();
            let v = json!({ "items": items });
            let f = unique(&v, "items[].name", "U", Severity::Error);
            let distinct = names.iter().collect::<BTreeSet<_>>().len();
            let expected_dups = names.len() - distinct;
            proptest::prop_assert_eq!(f.len(), expected_dups);
        }

        /// Property: `acyclic` produces no findings when all edges go
        /// from a lower-indexed node to a higher-indexed node (a valid
        /// topological order). Generated graphs cover sizes 2..15
        /// nodes and any subset of forward edges.
        #[test]
        fn acyclic_clean_on_topologically_ordered_edges(
            (n, edges) in dag_strategy(2usize..15)
        ) {
            let modules: Vec<serde_json::Value> = (0..n)
                .map(|i| {
                    let deps: Vec<String> = edges
                        .iter()
                        .filter(|(from, _)| *from == i)
                        .map(|(_, to)| format!("n{to}"))
                        .collect();
                    json!({ "name": format!("n{i}"), "depends_on": deps })
                })
                .collect();
            let v = json!({ "modules": modules });
            let f = acyclic(
                &v,
                "modules[].name",
                "modules[].depends_on[]",
                "A",
                Severity::Error,
            );
            proptest::prop_assert!(
                f.is_empty(),
                "DAG flagged as cyclic: edges={edges:?}, findings={f:?}",
            );
        }

        /// Property: injecting a back-edge (from higher index to lower)
        /// into an otherwise-DAG graph triggers an `acyclic` finding.
        #[test]
        fn acyclic_flags_back_edge(
            (n, mut edges) in dag_strategy(2usize..10)
        ) {
            // Add a back-edge from the highest indexed node to the
            // lowest (which must be reachable from n-1 transitively for
            // some topological orderings; we ensure reachability by
            // adding a forward edge 0→(n-1) too if it isn't there).
            let forward = (0, n - 1);
            if !edges.contains(&forward) {
                edges.push(forward);
            }
            edges.push((n - 1, 0));

            let modules: Vec<serde_json::Value> = (0..n)
                .map(|i| {
                    let deps: Vec<String> = edges
                        .iter()
                        .filter(|(from, _)| *from == i)
                        .map(|(_, to)| format!("n{to}"))
                        .collect();
                    json!({ "name": format!("n{i}"), "depends_on": deps })
                })
                .collect();
            let v = json!({ "modules": modules });
            let f = acyclic(
                &v,
                "modules[].name",
                "modules[].depends_on[]",
                "A",
                Severity::Error,
            );
            proptest::prop_assert!(
                !f.is_empty(),
                "back-edge graph not flagged: edges={edges:?}",
            );
        }
    }

    /// Strategy for a directed acyclic graph: returns `(n, edges)` where
    /// `n` is the node count and every edge `(from, to)` satisfies `from
    /// < to`. Used by the proptest cases above.
    fn dag_strategy(
        n_range: std::ops::Range<usize>,
    ) -> impl proptest::strategy::Strategy<Value = (usize, Vec<(usize, usize)>)> {
        use proptest::strategy::Strategy;
        n_range.prop_flat_map(|n| {
            // Generate a subset of all forward edges (i, j) with i < j.
            // Cap edge count proportional to n to keep the search space
            // tractable. The from-index is sampled from `0..n-1` because
            // the last node has no valid successor in the forward graph.
            let max_edges = (n * (n - 1) / 2).min(20);
            proptest::collection::vec(
                (0..n.saturating_sub(1))
                    .prop_flat_map(move |i| (proptest::strategy::Just(i), (i + 1)..n)),
                0..=max_edges,
            )
            .prop_map(move |edges| (n, edges))
        })
    }
}
