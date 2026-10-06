//! The requirement tree: a requirement names its parent with `satisfies`.
//!
//! A parent is covered by its children: a requirement with at least one live
//! child — a requirement of the same project, not retired, whose `satisfies`
//! names it — is exempt from "has a place" (`requirements_have_satisfiers`)
//! and "has a test" (`requirements_have_tests`). Each child is checked on its
//! own, so every leaf still needs a place and a test. A requirement on a cycle
//! of parent links is never exempt: a cycle has no leaf, and
//! [`RULE`] reports it. One [`Tree`] serves every reader of the two rules
//! (`validate`, `check`, `report coverage`) and `report requirements`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde_json::{json, Value};

use crate::domain::Severity;
use crate::entries::{is_retired_file, Entry, EntryKind, EntryStore};
use crate::layout::LayoutContext;
use crate::report::{Finding, Report};

pub const RULE: &str = "requirement_parents_acyclic";

/// The live requirements of one project and their parent links.
pub struct Tree {
    /// Live requirement id → its entry, in id order.
    live: BTreeMap<String, Entry>,
    /// Child id → its live parents, in id order.
    parents: BTreeMap<String, Vec<String>>,
    /// Parent id → its live children, in id order.
    children: BTreeMap<String, Vec<String>>,
    /// Retired requirement ids.
    retired: Vec<String>,
    /// The ids on a cycle of parent links, one set per cycle.
    cycles: Vec<BTreeSet<String>>,
}

impl Tree {
    pub fn load(store: &EntryStore) -> Self {
        let mut live = BTreeMap::new();
        let mut retired = Vec::new();
        for e in store.load_lenient(EntryKind::Requirement) {
            let Some(id) = e.id().map(str::to_string) else {
                continue;
            };
            if is_retired_file(&e.file) {
                retired.push(id);
            } else {
                live.entry(id).or_insert(e);
            }
        }
        retired.sort();
        let mut parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, e) in &live {
            for p in strings(e.value.get("satisfies")) {
                if live.contains_key(&p) {
                    parents.entry(id.clone()).or_default().push(p.clone());
                    children.entry(p).or_default().push(id.clone());
                }
            }
        }
        for v in children.values_mut().chain(parents.values_mut()) {
            v.sort();
            v.dedup();
        }
        let cycles = cycles(&live, &parents);
        Tree {
            live,
            parents,
            children,
            retired,
            cycles,
        }
    }

    pub fn for_layout(layout: &LayoutContext) -> Self {
        Self::load(&EntryStore::for_layout(layout))
    }

    /// The parents covered by their children: at least one live child, and
    /// not on a cycle.
    pub fn exempt(&self) -> BTreeSet<String> {
        let on_cycle: BTreeSet<&String> = self.cycles.iter().flatten().collect();
        self.children
            .keys()
            .filter(|p| !on_cycle.contains(p))
            .cloned()
            .collect()
    }

    /// One error per cycle of parent links, on its first id.
    pub fn findings(&self, rules: &BTreeMap<String, Severity>, report: &mut Report) {
        let sev = rules.get(RULE).copied().unwrap_or(Severity::Error);
        for c in &self.cycles {
            let ids: Vec<&str> = c.iter().map(String::as_str).collect();
            let first = ids[0];
            let mut f = Finding::new(
                RULE,
                sev,
                format!(
                    "requirements {} name each other as parents (`satisfies`) in a cycle: a cycle has no leaf, \
                     so none of them is covered by its children; break the cycle",
                    ids.iter().map(|i| format!("`{i}`")).collect::<Vec<_>>().join(", ")
                ),
            )
            .with_entity(first);
            if let Some(e) = self.live.get(first) {
                f = f.with_file(e.file.display().to_string());
            }
            report.push(f);
        }
    }

    /// The tree as YAML-ready JSON: `requirements` (roots, each with its
    /// children) and `retired`. `modules` maps a requirement id to the
    /// modules that satisfy it.
    pub fn to_value(&self, modules: &BTreeMap<String, Vec<String>>) -> Value {
        let roots = self.roots();
        let nodes: Vec<Value> = roots
            .iter()
            .map(|r| self.node(r, modules, &mut Vec::new()))
            .collect();
        json!({ "requirements": nodes, "retired": self.retired })
    }

    /// The tree as an indented outline, two spaces per level.
    pub fn to_text(&self, modules: &BTreeMap<String, Vec<String>>) -> String {
        let mut out = String::new();
        for r in self.roots() {
            self.line(&r, 0, modules, &mut Vec::new(), &mut out);
        }
        if !self.retired.is_empty() {
            out.push_str(&format!("retired: {}\n", self.retired.join(", ")));
        }
        out
    }

    /// Requirements with no live parent, then any requirement not reached
    /// from them (the members of a cycle with no root).
    fn roots(&self) -> Vec<String> {
        let mut roots: Vec<String> = self
            .live
            .keys()
            .filter(|id| !self.parents.contains_key(*id))
            .cloned()
            .collect();
        let mut seen = BTreeSet::new();
        for r in &roots {
            self.reach(r, &mut seen);
        }
        for id in self.live.keys() {
            if !seen.contains(id) {
                roots.push(id.clone());
                self.reach(id, &mut seen);
            }
        }
        roots
    }

    fn reach(&self, id: &str, seen: &mut BTreeSet<String>) {
        if !seen.insert(id.to_string()) {
            return;
        }
        for c in self.children.get(id).into_iter().flatten() {
            self.reach(c, seen);
        }
    }

    fn node(
        &self,
        id: &str,
        modules: &BTreeMap<String, Vec<String>>,
        path: &mut Vec<String>,
    ) -> Value {
        let e = &self.live[id];
        let flag = |k: &str| e.value.get(k).and_then(toml::Value::as_bool) == Some(true);
        let mut n = json!({
            "id": id,
            "satisfied_by": modules.get(id).cloned().unwrap_or_default(),
            "verified_by": strings(e.value.get("verified_by")),
        });
        if flag("planned") {
            n["planned"] = json!(true);
        }
        if flag("test_pending") {
            n["test_pending"] = json!(true);
        }
        if let Some(ps) = self.parents.get(id).filter(|p| p.len() > 1) {
            n["parents"] = json!(ps);
        }
        if path.iter().any(|p| p == id) {
            n["cycle"] = json!(true);
            return n;
        }
        path.push(id.to_string());
        let kids: Vec<Value> = self
            .children
            .get(id)
            .into_iter()
            .flatten()
            .map(|c| self.node(c, modules, path))
            .collect();
        path.pop();
        if !kids.is_empty() {
            n["children"] = Value::Array(kids);
        }
        n
    }

    fn line(
        &self,
        id: &str,
        depth: usize,
        modules: &BTreeMap<String, Vec<String>>,
        path: &mut Vec<String>,
        out: &mut String,
    ) {
        let e = &self.live[id];
        let mut parts = vec![format!("{}{id}", "  ".repeat(depth))];
        if let Some(m) = modules.get(id).filter(|m| !m.is_empty()) {
            parts.push(format!("modules: {}", m.join(", ")));
        }
        let tests = strings(e.value.get("verified_by"));
        if !tests.is_empty() {
            parts.push(format!("tests: {}", tests.join(", ")));
        }
        if self.parents.get(id).is_some_and(|p| p.len() > 1) {
            parts.push(format!("parents: {}", self.parents[id].join(", ")));
        }
        let on_path = path.iter().any(|p| p == id);
        if on_path {
            parts.push("(cycle)".to_string());
        }
        out.push_str(&parts.join("  "));
        out.push('\n');
        if on_path {
            return;
        }
        path.push(id.to_string());
        for c in self.children.get(id).into_iter().flatten() {
            self.line(c, depth + 1, modules, path, out);
        }
        path.pop();
    }
}

/// The modules that satisfy each requirement (module level and `items[]`).
pub fn modules_by_requirement(layout: &LayoutContext) -> Result<BTreeMap<String, Vec<String>>> {
    Ok(crate::coverage::build(layout)?
        .matrix
        .into_iter()
        .map(|row| (row.requirement, row.satisfied_by))
        .collect())
}

/// Strongly connected components of the child → parent graph with more than
/// one member, or a requirement that names itself.
fn cycles(
    live: &BTreeMap<String, Entry>,
    parents: &BTreeMap<String, Vec<String>>,
) -> Vec<BTreeSet<String>> {
    struct S<'a> {
        parents: &'a BTreeMap<String, Vec<String>>,
        index: BTreeMap<String, usize>,
        low: BTreeMap<String, usize>,
        stack: Vec<String>,
        on: BTreeSet<String>,
        next: usize,
        out: Vec<BTreeSet<String>>,
    }
    fn visit(s: &mut S, v: &str) {
        s.index.insert(v.to_string(), s.next);
        s.low.insert(v.to_string(), s.next);
        s.next += 1;
        s.stack.push(v.to_string());
        s.on.insert(v.to_string());
        for w in s.parents.get(v).cloned().unwrap_or_default() {
            if !s.index.contains_key(&w) {
                visit(s, &w);
                let lw = s.low[&w];
                let lv = s.low.get_mut(v).unwrap();
                *lv = (*lv).min(lw);
            } else if s.on.contains(&w) {
                let iw = s.index[&w];
                let lv = s.low.get_mut(v).unwrap();
                *lv = (*lv).min(iw);
            }
        }
        if s.low[v] == s.index[v] {
            let mut comp = BTreeSet::new();
            while let Some(w) = s.stack.pop() {
                s.on.remove(&w);
                let done = w == v;
                comp.insert(w);
                if done {
                    break;
                }
            }
            let self_loop = s.parents.get(v).is_some_and(|p| p.iter().any(|x| x == v));
            if comp.len() > 1 || self_loop {
                s.out.push(comp);
            }
        }
    }
    let mut s = S {
        parents,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        stack: Vec::new(),
        on: BTreeSet::new(),
        next: 0,
        out: Vec::new(),
    };
    for id in live.keys() {
        if !s.index.contains_key(id) {
            visit(&mut s, id);
        }
    }
    s.out.sort();
    s.out
}

fn strings(v: Option<&toml::Value>) -> Vec<String> {
    v.and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect()
}
