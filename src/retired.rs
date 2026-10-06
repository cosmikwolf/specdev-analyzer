//! Retired requirements (D117).
//!
//! A requirement is retired by moving its file to
//! `specs/requirements/retired/` with `retired_by = "DEC-NNN"`. The folder is
//! the state; the field says why. [`crate::entries`] and the reducer still
//! read the folder, so ids stay unique and links to a retired id resolve,
//! and every retired entity carries `retired = true`. This module holds the
//! spec-only checks `validate` runs, and the schema admission that lets a
//! retired file carry the field whatever the project's inlined schema says.
//!
//! Rules:
//! - [`RULE`] (error): `retired_by` missing in `retired/` or present outside
//!   it; `retired_by` names no decision, or one whose supersession chain does
//!   not end at an accepted decision; a module (module-level or `items[]`
//!   `satisfies`) or a live requirement (`satisfies`, a parent link) builds on
//!   a retired requirement.
//! - [`OQ_RULE`] (info): an open question whose `affects` names only retired
//!   requirements gates nothing.
//! - [`TESTED`] (warning, run by `check`): a retired requirement whose
//!   `verified_by` still names a discovered test.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::domain::Severity;
use crate::entries::{is_retired_file, Entry, EntryKind, EntryStore};
use crate::layout::LayoutContext;
use crate::report::{Finding, Report};

pub const RULE: &str = "retired_requirement_consistency";
pub const OQ_RULE: &str = "open_question_affects_only_retired";
pub const TESTED: &str = "retired_requirement_tested";

/// The requirement field that names what retired it.
pub const FIELD: &str = "retired_by";

/// Let a retired requirement's file carry `retired_by` whatever the inlined
/// schema says (D117, review H1), as `admit_planned` does for `planned`.
pub fn admit_retired_by(schema: &mut Value) {
    if let Some(props) = schema
        .pointer_mut("/$defs/Requirement/properties")
        .and_then(Value::as_object_mut)
    {
        props
            .entry(FIELD.to_string())
            .or_insert_with(|| serde_json::json!({ "type": "string", "minLength": 1 }));
    }
}

/// The ids of the retired requirements in `store`.
pub fn retired_ids(store: &EntryStore) -> BTreeSet<String> {
    store
        .load_lenient(EntryKind::Requirement)
        .iter()
        .filter(|e| is_retired_file(&e.file))
        .filter_map(|e| e.id().map(str::to_string))
        .collect()
}

/// The spec-only checks; `rules` are the project's severities.
pub fn spec_checks(
    layout: &LayoutContext,
    rules: &BTreeMap<String, Severity>,
    report: &mut Report,
) {
    let store = EntryStore::for_layout(layout);
    let sev = |rule: &str, default: Severity| rules.get(rule).copied().unwrap_or(default);
    let err = sev(RULE, Severity::Error);
    let requirements = store.load_lenient(EntryKind::Requirement);
    let retired: BTreeSet<&str> = requirements
        .iter()
        .filter(|e| is_retired_file(&e.file))
        .filter_map(Entry::id)
        .collect();
    let decisions = store.load_lenient(EntryKind::Decision);
    let push = |report: &mut Report, entity: &str, file: &Path, msg: String| {
        report.push(
            Finding::new(RULE, err, msg)
                .with_file(file.display().to_string())
                .with_entity(entity),
        );
    };

    for e in &requirements {
        let Some(id) = e.id() else { continue };
        let by = e.value.get(FIELD).and_then(toml::Value::as_str);
        match (is_retired_file(&e.file), by) {
            (true, None) => push(
                report,
                id,
                &e.file,
                format!(
                    "requirement `{id}` is in `specs/requirements/retired/`, so it needs `{FIELD}` naming the \
                     accepted decision that retired it; or move it back if it is live"
                ),
            ),
            (false, Some(_)) => push(
                report,
                id,
                &e.file,
                format!(
                    "requirement `{id}` has `{FIELD}`, which belongs only in `specs/requirements/retired/`: \
                     move the file there to retire it, or remove the line to keep it live"
                ),
            ),
            (true, Some(dec)) => {
                if let Some(why) = not_standing(dec, &decisions) {
                    push(
                        report,
                        id,
                        &e.file,
                        format!("requirement `{id}` is retired by `{dec}`, but {why}"),
                    );
                }
            }
            (false, None) => {
                for parent in strings(e.value.get("satisfies")) {
                    if retired.contains(parent.as_str()) {
                        push(
                            report,
                            id,
                            &e.file,
                            format!(
                                "requirement `{id}` satisfies `{parent}`, which is retired: retire `{id}` \
                                 too, or point it at a live parent"
                            ),
                        );
                    }
                }
            }
        }
    }

    for (module, file, claims) in module_claims(layout) {
        for r in claims.iter().filter(|r| retired.contains(r.as_str())) {
            push(
                report,
                &module,
                &file,
                format!(
                    "module `{module}` satisfies `{r}`, which is retired: remove it from `satisfies` \
                     (module level or `items[]`)"
                ),
            );
        }
    }

    let info = sev(OQ_RULE, Severity::Info);
    for q in store.load_lenient(EntryKind::OpenQuestion) {
        let Some(id) = q.id() else { continue };
        if q.value.get("status").and_then(toml::Value::as_str) != Some("open") {
            continue;
        }
        let affects = strings(q.value.get("affects"));
        if !affects.is_empty() && affects.iter().all(|r| retired.contains(r.as_str())) {
            report.push(
                Finding::new(
                    OQ_RULE,
                    info,
                    format!(
                        "open question `{id}` affects only retired requirements ({}), so it gates nothing; \
                         close it or point it at live work",
                        affects.join(", ")
                    ),
                )
                .with_file(q.file.display().to_string())
                .with_entity(id),
            );
        }
    }
}

/// Why decision `id` does not stand behind a retirement, or `None` when it
/// is accepted or superseded through a chain that ends at an accepted one.
fn not_standing(id: &str, decisions: &[Entry]) -> Option<String> {
    let status = |d: &str| -> Option<String> {
        decisions.iter().find(|e| e.id() == Some(d)).map(|e| {
            e.value
                .get("status")
                .and_then(toml::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
    };
    let successors = |d: &str| -> Vec<String> {
        decisions
            .iter()
            .filter(|e| strings(e.value.get("supersedes")).iter().any(|s| s == d))
            .filter_map(|e| e.id().map(str::to_string))
            .collect()
    };
    let Some(first) = status(id) else {
        return Some("no decision has that id".to_string());
    };
    let mut seen = BTreeSet::new();
    let mut todo = vec![(id.to_string(), first)];
    while let Some((d, st)) = todo.pop() {
        if !seen.insert(d.clone()) {
            continue;
        }
        match st.as_str() {
            "accepted" => return None,
            "superseded" => {
                for s in successors(&d) {
                    if let Some(st) = status(&s) {
                        todo.push((s, st));
                    }
                }
            }
            _ => {}
        }
    }
    Some(format!(
        "`{id}` is not accepted, and no chain of decisions that supersede it ends at an accepted one"
    ))
}

/// Each module spec: its name, its file, and every requirement id it claims
/// (`satisfies` at module level and in `items[]`).
fn module_claims(layout: &LayoutContext) -> Vec<(String, std::path::PathBuf, Vec<String>)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&layout.modules_dir) else {
        return out;
    };
    let mut files: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(doc) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        let Some(name) = doc.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let mut claims = strings(doc.get("satisfies"));
        for item in doc
            .get("items")
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            claims.extend(strings(item.get("satisfies")));
        }
        out.push((name.to_string(), file, claims));
    }
    out
}

fn strings(v: Option<&toml::Value>) -> Vec<String> {
    v.and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|s| s.as_str().map(str::to_string))
        .collect()
}
