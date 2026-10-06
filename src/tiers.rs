//! Ids across the workspace tier and its members.
//!
//! A member reads the workspace root's decisions and open questions as a
//! second tier and merges the two by id. An id in both tiers therefore hides
//! one entry; [`member_checks`] makes that an error, and the member's own
//! entry is the one its context uses while the error stands.
//!
//! Requirement ids are each member's own: the same id in two members is
//! normal. It is a hazard only where an id crosses members — a workspace open
//! question whose `affects` names it gates every member that defines it.
//! [`root_checks`] warns about that, and about an `affects` id no member
//! defines (the question then gates nothing).

use std::collections::BTreeMap;

use crate::domain::Severity;
use crate::entries::{Entry, EntryKind, EntryStore};
use crate::layout::LayoutContext;
use crate::report::{Finding, Report};

pub const COLLISION: &str = "workspace_id_collision";
pub const AMBIGUOUS: &str = "workspace_affects_ambiguous";
pub const NOTHING: &str = "workspace_affects_nothing";

/// In a workspace member: each decision or open question whose id the
/// workspace tier also uses.
pub fn member_checks(
    layout: &LayoutContext,
    rules: &BTreeMap<String, Severity>,
    report: &mut Report,
) {
    let Ok(Some(root)) = EntryStore::workspace_root_of(layout) else {
        return;
    };
    let local = EntryStore::for_layout(layout);
    let sev = rules.get(COLLISION).copied().unwrap_or(Severity::Error);
    for kind in [EntryKind::Decision, EntryKind::OpenQuestion] {
        let theirs: BTreeMap<String, Entry> = root
            .load_lenient(kind)
            .into_iter()
            .filter_map(|e| e.id().map(str::to_string).map(|id| (id, e)))
            .collect();
        for mine in local.load_lenient(kind) {
            let Some(id) = mine.id() else { continue };
            let Some(other) = theirs.get(id) else {
                continue;
            };
            report.push(
                Finding::new(
                    COLLISION,
                    sev,
                    format!(
                        "`{id}` (\"{}\") has the id of the workspace's `{id}` (\"{}\", {}); the two tiers are \
                         merged by id, so one hides the other: rename one of them",
                        title(&mine),
                        title(other),
                        other.file.display()
                    ),
                )
                .with_file(mine.file.display().to_string())
                .with_entity(id),
            );
        }
    }
}

/// At a workspace root: each open question (not closed) whose `affects`
/// names an id that two or more members define, or that no member defines.
pub fn root_checks(
    layout: &LayoutContext,
    rules: &BTreeMap<String, Severity>,
    report: &mut Report,
) {
    let Some(ws) = layout.spec.workspace.as_ref() else {
        return;
    };
    // Requirement id → the members (paths) that define it.
    let mut defined: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in &ws.members {
        let spec = layout.project_root.join(&m.path).join("spec.toml");
        let Ok(store) = EntryStore::for_spec_toml(&spec) else {
            continue;
        };
        for e in store.load_lenient(EntryKind::Requirement) {
            if let Some(id) = e.id() {
                defined
                    .entry(id.to_string())
                    .or_default()
                    .push(m.path.clone());
            }
        }
    }
    let sev = |r: &str| rules.get(r).copied().unwrap_or(Severity::Warning);
    for q in EntryStore::for_layout(layout).load_lenient(EntryKind::OpenQuestion) {
        let Some(id) = q.id() else { continue };
        if q.value.get("status").and_then(toml::Value::as_str) == Some("closed") {
            continue;
        }
        let affects = q
            .value
            .get("affects")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str);
        for r in affects {
            let (rule, msg) = match defined.get(r).map(Vec::as_slice) {
                None | Some([]) => (
                    NOTHING,
                    format!(
                        "workspace open question `{id}` affects `{r}`, which no member defines, so it gates nothing \
                         for it"
                    ),
                ),
                Some([_]) => continue,
                Some(members) => (
                    AMBIGUOUS,
                    format!(
                        "workspace open question `{id}` affects `{r}`, which {} members define ({}); it gates \
                         every one of them — give each member's requirement its own id if they differ",
                        members.len(),
                        members.join(", ")
                    ),
                ),
            };
            report.push(
                Finding::new(rule, sev(rule), msg)
                    .with_file(q.file.display().to_string())
                    .with_entity(id),
            );
        }
    }
}

fn title(e: &Entry) -> &str {
    e.value
        .get("title")
        .and_then(toml::Value::as_str)
        .unwrap_or_default()
}
