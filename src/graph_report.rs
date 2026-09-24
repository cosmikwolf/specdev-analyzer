//! `report graph` — the module dependency graph as Mermaid or DOT.
//!
//! A pure projection over the reduction, the same posture as `context`: no
//! new schema, no new graph primitive. The grouping comes from the project's
//! own data, never from a name in this crate. When `domain.toml` declares a
//! `layer_dependency` graph check, the report takes that check's
//! `entity_scope`, `relation_scope`, `layer_field` and `layer_ordering`: nodes
//! are grouped by layer in the declared order, and an edge the check flags is
//! drawn in a distinct style. The flagging is the check's own rule
//! ([`graph::edge_direction`]), so the picture and `validate` cannot disagree.
//! Without such a check the canonical `Module` / `depends_on` pair is used and
//! the graph is not grouped. A node is an entity a spec file declares at its
//! root; block entities nested inside a module file are left out.
//!
//! Output is deterministic — nodes by layer order then name, edges sorted, no
//! timestamp — so a committed map diffs cleanly when an edge is added.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{anyhow, bail, Context, Result};

use crate::domain::{DomainConfig, GraphCheck};
use crate::graph::{self, EdgeDirection};
use crate::layout::LayoutContext;

/// The canonical pair used when the project declares no `layer_dependency`
/// check. Both are canonical kinds every profile's reduction extends.
const DEFAULT_ENTITY_SCOPE: &str = "Module";
const DEFAULT_RELATION_SCOPE: &str = "depends_on";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphFormat {
    Mermaid,
    Dot,
}

impl GraphFormat {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "mermaid" => Ok(Self::Mermaid),
            "dot" => Ok(Self::Dot),
            other => bail!("unknown graph format `{other}`; use `mermaid` or `dot`"),
        }
    }
}

/// The projected graph, ready to render.
#[derive(Debug, Clone)]
pub struct GraphView {
    /// The graph check the grouping came from, if any.
    pub check_rule: Option<String>,
    pub layer_field: Option<String>,
    /// Groups in render order: the declared layers first (only those with
    /// nodes), then any layer not in the ordering, then ungrouped nodes.
    pub groups: Vec<Group>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone)]
pub struct Group {
    /// `None` for nodes without a layer (or when nothing groups).
    pub layer: Option<String>,
    /// True when `layer` is not in the declared ordering.
    pub unknown: bool,
    pub nodes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// The `layer_dependency` check flags this edge.
    pub violation: bool,
}

impl GraphView {
    pub fn node_count(&self) -> usize {
        self.groups.iter().map(|g| g.nodes.len()).sum()
    }

    pub fn violation_count(&self) -> usize {
        self.edges.iter().filter(|e| e.violation).count()
    }
}

/// Build the view. `check` names the `layer_dependency` rule to group by
/// when the project declares more than one; `None` takes the first.
pub fn build(layout: &LayoutContext, check: Option<&str>) -> Result<GraphView> {
    let inv = crate::reductions::load_all(layout)?;
    let (kindset, _) = crate::reductions::resolve(&inv);
    let mut exec_report = crate::report::Report::default();
    let graph = crate::reduction::execute_kindset(&kindset, layout, &mut exec_report)?;
    let graph_json = serde_json::to_value(&graph).context("serializing the entity graph")?;

    let layered = layer_check(layout, check)?;
    let (entity_scope, relation_scope) = match &layered {
        Some(c) => (
            c.entity_scope.as_deref().unwrap_or(DEFAULT_ENTITY_SCOPE),
            c.relation_scope
                .as_deref()
                .unwrap_or(DEFAULT_RELATION_SCOPE),
        ),
        None => (DEFAULT_ENTITY_SCOPE, DEFAULT_RELATION_SCOPE),
    };
    let view = crate::validate::filter_for_layer_dependency(
        &graph_json,
        entity_scope,
        relation_scope,
        &kindset,
    );

    // A node is an entity a spec file declares at its root — a module.
    // Block kinds may extend the module kind too (a task, a peripheral, a
    // datasource), but they come from a nested table of a module file
    // (`/embassy/tasks/0`); on a module map they are noise.
    let names: BTreeSet<String> = view
        .get("entities")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|e| e.pointer("/provenance/pointer").and_then(|p| p.as_str()) == Some(""))
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();

    let layer_field = layered.as_ref().and_then(|c| c.layer_field.clone());
    let ordering: Vec<String> = layered
        .as_ref()
        .and_then(|c| c.layer_ordering.clone())
        .unwrap_or_default();
    let layers = match &layer_field {
        Some(f) => graph::entity_layers(&view, f),
        None => Default::default(),
    };

    // Edges between two nodes in view only: a relation kind that extends
    // `depends_on` can also join other entity kinds.
    let edges: BTreeSet<Edge> = graph::relation_edges(&view)
        .into_iter()
        .filter(|e| names.contains(&e.from) && names.contains(&e.to))
        .map(|e| {
            let violation = match (layers.get(&e.from), layers.get(&e.to)) {
                (Some(f), Some(t)) => {
                    graph::edge_direction(&ordering, f, t) == EdgeDirection::Upward
                }
                _ => false,
            };
            Edge {
                from: e.from,
                to: e.to,
                violation,
            }
        })
        .collect();

    let mut by_layer: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    for n in &names {
        by_layer
            .entry(layers.get(n).cloned())
            .or_default()
            .push(n.clone());
    }
    let mut groups = Vec::new();
    for l in &ordering {
        if let Some(nodes) = by_layer.remove(&Some(l.clone())) {
            groups.push(Group {
                layer: Some(l.clone()),
                unknown: false,
                nodes,
            });
        }
    }
    let ungrouped = by_layer.remove(&None);
    for (l, nodes) in by_layer {
        groups.push(Group {
            layer: l,
            unknown: true,
            nodes,
        });
    }
    if let Some(nodes) = ungrouped {
        groups.push(Group {
            layer: None,
            unknown: false,
            nodes,
        });
    }

    Ok(GraphView {
        check_rule: layered.and_then(|c| c.rule),
        layer_field,
        groups,
        edges: edges.into_iter().collect(),
    })
}

/// The project's `layer_dependency` check with a layer field and an
/// ordering: the named one, else the first declared.
fn layer_check(layout: &LayoutContext, name: Option<&str>) -> Result<Option<GraphCheck>> {
    if !layout.domain_toml_path.is_file() {
        return match name {
            Some(n) => Err(anyhow!("--check `{n}`: the project has no domain.toml")),
            None => Ok(None),
        };
    }
    let dom = DomainConfig::load(&layout.domain_toml_path)?;
    let mut candidates = dom.project.graph_checks.into_iter().filter(|c| {
        c.kind == "layer_dependency" && c.layer_field.is_some() && c.layer_ordering.is_some()
    });
    match name {
        None => Ok(candidates.next()),
        Some(n) => candidates
            .find(|c| c.rule.as_deref() == Some(n))
            .map(Some)
            .ok_or_else(|| {
                anyhow!(
                    "--check `{n}`: no `layer_dependency` graph check with that rule, a \
                     `layer_field` and a `layer_ordering` in {}",
                    layout.domain_toml_path.display()
                )
            }),
    }
}

fn degrees(view: &GraphView) -> (BTreeMap<&str, usize>, BTreeMap<&str, usize>) {
    let mut uses: BTreeMap<&str, usize> = BTreeMap::new();
    let mut used_by: BTreeMap<&str, usize> = BTreeMap::new();
    for e in &view.edges {
        *uses.entry(e.from.as_str()).or_default() += 1;
        *used_by.entry(e.to.as_str()).or_default() += 1;
    }
    (uses, used_by)
}

fn group_title(g: &Group) -> String {
    match (&g.layer, g.unknown) {
        (Some(l), false) => l.clone(),
        (Some(l), true) => format!("{l} (not in the layer ordering)"),
        (None, _) => "(no layer)".to_string(),
    }
}

fn header_lines(view: &GraphView) -> Vec<String> {
    let mut lines = vec!["generated by `specdev-analyzer report graph`; do not edit".to_string()];
    match (&view.check_rule, &view.layer_field) {
        (Some(rule), Some(field)) => lines.push(format!(
            "grouped by `{field}` in the order of graph check `{rule}`; a thick red edge is one it flags"
        )),
        _ => lines.push("no `layer_dependency` graph check declared: not grouped".to_string()),
    }
    lines.push("node label: name, then how many modules it uses and how many use it".to_string());
    lines
}

/// Mermaid `flowchart`. Node ids are positional (`n0`, `n1`, ...) so any
/// name renders; the name itself is the quoted label.
pub fn render_mermaid(view: &GraphView) -> String {
    let (uses, used_by) = degrees(view);
    let mut ids: BTreeMap<&str, String> = BTreeMap::new();
    let mut out = String::new();
    for l in header_lines(view) {
        let _ = writeln!(out, "%% {l}");
    }
    out.push_str("flowchart TB\n");
    let mut next = 0usize;
    for (gi, g) in view.groups.iter().enumerate() {
        let grouped = g.layer.is_some() || view.groups.len() > 1;
        if grouped {
            let _ = writeln!(
                out,
                "  subgraph g{gi}[\"{}\"]",
                mermaid_text(&group_title(g))
            );
        }
        for n in &g.nodes {
            let id = format!("n{next}");
            next += 1;
            let _ = writeln!(
                out,
                "  {}{id}[\"{}<br/>uses {} · used by {}\"]",
                if grouped { "  " } else { "" },
                mermaid_text(n),
                uses.get(n.as_str()).copied().unwrap_or(0),
                used_by.get(n.as_str()).copied().unwrap_or(0),
            );
            ids.insert(n.as_str(), id);
        }
        if grouped {
            out.push_str("  end\n");
        }
    }
    // `linkStyle` indexes links in the order they are written, so count
    // the written ones, not positions in `view.edges`.
    let mut flagged = Vec::new();
    let mut written = 0usize;
    for e in &view.edges {
        let (Some(a), Some(b)) = (ids.get(e.from.as_str()), ids.get(e.to.as_str())) else {
            continue;
        };
        if e.violation {
            let _ = writeln!(out, "  {a} ==>|upward| {b}");
            flagged.push(written.to_string());
        } else {
            let _ = writeln!(out, "  {a} --> {b}");
        }
        written += 1;
    }
    // Invisible links chain the layer groups so they stack top-down in the
    // declared order; without them Mermaid places subgraphs freely. Written
    // after every real edge so they do not shift the `linkStyle` indexes.
    let grouped: Vec<String> = view
        .groups
        .iter()
        .enumerate()
        .filter(|(_, g)| g.layer.is_some() || view.groups.len() > 1)
        .map(|(i, _)| format!("g{i}"))
        .collect();
    if grouped.len() > 1 {
        let _ = writeln!(out, "  {}", grouped.join(" ~~~ "));
    }
    if !flagged.is_empty() {
        let _ = writeln!(
            out,
            "  linkStyle {} stroke:#d00,stroke-width:3px",
            flagged.join(",")
        );
    }
    out
}

/// Graphviz DOT: one `cluster` per layer, flagged edges red and bold.
pub fn render_dot(view: &GraphView) -> String {
    let (uses, used_by) = degrees(view);
    let mut out = String::new();
    for l in header_lines(view) {
        let _ = writeln!(out, "// {l}");
    }
    out.push_str("digraph modules {\n  rankdir=TB;\n  node [shape=box];\n");
    for (gi, g) in view.groups.iter().enumerate() {
        let grouped = g.layer.is_some() || view.groups.len() > 1;
        let indent = if grouped { "    " } else { "  " };
        if grouped {
            let _ = writeln!(
                out,
                "  subgraph cluster_{gi} {{\n    label={};",
                dot_quote(&group_title(g))
            );
        }
        for n in &g.nodes {
            let _ = writeln!(
                out,
                "{indent}{} [label={}];",
                dot_quote(n),
                dot_quote(&format!(
                    "{n}\nuses {} · used by {}",
                    uses.get(n.as_str()).copied().unwrap_or(0),
                    used_by.get(n.as_str()).copied().unwrap_or(0)
                ))
            );
        }
        if grouped {
            out.push_str("  }\n");
        }
    }
    for e in &view.edges {
        let style = if e.violation {
            " [color=red, penwidth=3, label=\"upward\"]"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "  {} -> {}{style};",
            dot_quote(&e.from),
            dot_quote(&e.to)
        );
    }
    out.push_str("}\n");
    out
}

/// Text inside a Mermaid quoted label: `"` and the markup characters
/// become entity codes, so a qualified name (`a::B<T>`) renders as written.
fn mermaid_text(s: &str) -> String {
    s.replace('&', "#amp;")
        .replace('"', "#quot;")
        .replace('<', "#lt;")
        .replace('>', "#gt;")
}

fn dot_quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> GraphView {
        GraphView {
            check_rule: Some("layers".into()),
            layer_field: Some("attrs.layer".into()),
            groups: vec![
                Group {
                    layer: Some("top".into()),
                    unknown: false,
                    nodes: vec!["App".into()],
                },
                Group {
                    layer: Some("low".into()),
                    unknown: false,
                    nodes: vec!["Db\"x<T>".into()],
                },
            ],
            edges: vec![
                Edge {
                    from: "App".into(),
                    to: "Db\"x<T>".into(),
                    violation: false,
                },
                Edge {
                    from: "Db\"x<T>".into(),
                    to: "App".into(),
                    violation: true,
                },
            ],
        }
    }

    #[test]
    fn mermaid_groups_escapes_and_styles_the_flagged_edge() {
        let m = render_mermaid(&view());
        assert!(m.contains("subgraph g0[\"top\"]"), "{m}");
        assert!(
            m.contains("n1[\"Db#quot;x#lt;T#gt;<br/>uses 1 · used by 1\"]"),
            "{m}"
        );
        assert!(m.contains("n0 --> n1"), "{m}");
        assert!(m.contains("n1 ==>|upward| n0"), "{m}");
        assert!(m.contains("linkStyle 1 stroke:#d00"), "{m}");
    }

    #[test]
    fn dot_clusters_quote_and_color_the_flagged_edge() {
        let d = render_dot(&view());
        assert!(
            d.contains("subgraph cluster_1 {\n    label=\"low\";"),
            "{d}"
        );
        assert!(d.contains("\"Db\\\"x<T>\" -> \"App\" [color=red"), "{d}");
    }
}
