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
pub fn unique(
    root: &Value,
    field: &str,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let picks = select(root, field);
    let mut seen: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    for p in picks {
        let Some(s) = value_as_str(&p.value) else {
            continue;
        };
        if let Some(first) = seen.get(s) {
            out.push(
                Finding::new(
                    rule,
                    severity,
                    format!("duplicate value `{s}` (first seen at {first})"),
                )
                .with_pointer(p.pointer.clone()),
            );
        } else {
            seen.insert(s.to_string(), p.pointer);
        }
    }
    out
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
        root, nodes_field, edges_from, edges_to, entry_field, None, rule, severity,
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
        root, nodes_field, edges_from, edges_to, entry_field, Some(nth), rule, severity,
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
                    .filter(|p| {
                        ancestor_at_array(&p.pointer, n).as_deref() == Some(&g)
                    })
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

/// ACYCLIC: the directed graph defined by (edges_from, edges_to) has no cycles.
pub fn acyclic(
    root: &Value,
    edges_from: &str,
    edges_to: &str,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let froms = select(root, edges_from);
    let tos = select(root, edges_to);
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for (a, b) in froms.iter().zip(tos.iter()) {
        let (Some(f), Some(t)) = (a.value.as_str(), b.value.as_str()) else {
            continue;
        };
        adj.entry(f.to_string()).or_default().push(t.to_string());
    }
    // Kahn's algorithm on a distinct-node subgraph derived from edges.
    let mut nodes: BTreeSet<String> = BTreeSet::new();
    for (f, ts) in &adj {
        nodes.insert(f.clone());
        for t in ts {
            nodes.insert(t.clone());
        }
    }
    let mut indeg: HashMap<String, usize> =
        nodes.iter().map(|n| (n.clone(), 0)).collect();
    for ts in adj.values() {
        for t in ts {
            *indeg.entry(t.clone()).or_insert(0) += 1;
        }
    }
    let mut q: VecDeque<String> =
        indeg.iter().filter(|(_, &d)| d == 0).map(|(n, _)| n.clone()).collect();
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

/// LAYER_DEPENDENCY: for each module, every dependency's layer must be in the
/// depending module's allowed_deps list.
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
    fn reference_exists_flags_missing() {
        let v = json!({ "src": [{ "x": "b" }], "tgt": [{ "y": "a" }] });
        let f = reference_exists(&v, "src[].x", "tgt[].y", "R1", Severity::Error);
        assert_eq!(f.len(), 1);
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
}
