//! `export`: the project's decisions, requirements and open questions as
//! Markdown, for publication in the code root of a project whose specs are
//! private (the code cites ids a reader must be able to open).
//!
//! Layout, the same for every project, under `[export] path`:
//! `decisions/<root>/<ID>.md`, `requirements/<root>.md`,
//! `open_questions/<root>.md`, where `<root>` is the project's name and, at
//! a workspace root, each member's name too. Every id has an anchor
//! `<a id="<lower-case id>"></a>`. The bytes depend only on the spec: no
//! time stamp, every list sorted. `export` owns those three folders.
//!
//! A decision's `record` (process notes) is never exported; `[export] omit`
//! leaves out more fields per kind.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::ExportSection;
use crate::entries::{Entry, EntryKind, EntryStore};
use crate::layout::LayoutContext;

/// The folders `export` owns under the export path.
pub const OWNED_DIRS: [&str; 3] = ["decisions", "requirements", "open_questions"];

/// Fields a decision never exports.
const NEVER_EXPORTED: [&str; 1] = ["record"];

/// One spec root whose entries are exported: the project, or a member.
pub struct Root {
    pub name: String,
    pub store: EntryStore,
}

/// The project's own root and, at a workspace root, every member's.
pub fn roots(layout: &LayoutContext) -> Result<Vec<Root>> {
    let mut out = vec![Root {
        name: layout.spec.project.name.clone(),
        store: EntryStore::for_layout(layout),
    }];
    if let Some(ws) = &layout.spec.workspace {
        for m in &ws.members {
            let spec_toml = layout.project_root.join(&m.path).join("spec.toml");
            out.push(Root {
                name: m.name.clone(),
                store: EntryStore::for_spec_toml(&spec_toml)
                    .with_context(|| format!("member `{}`", m.name))?,
            });
        }
    }
    Ok(out)
}

/// The export folder, checked to stay inside the code root.
pub fn export_dir(layout: &LayoutContext, export: &ExportSection) -> Result<PathBuf> {
    let rel = Path::new(&export.path);
    let escapes = rel.is_absolute()
        || rel.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if escapes {
        bail!(
            "[EXPORT-002] `[export] path = \"{}\"` must be a relative path inside the code root",
            export.path
        );
    }
    Ok(layout.code_root.join(rel))
}

/// One entry, ready to render.
struct Item {
    id: String,
    value: toml::Value,
}

fn items(entries: Vec<Entry>) -> Vec<Item> {
    let mut v: Vec<Item> = entries
        .into_iter()
        .filter_map(|e| {
            let id = e.id()?.to_string();
            Some(Item { id, value: e.value })
        })
        .collect();
    v.sort_by_key(|a| id_key(&a.id));
    v.dedup_by(|a, b| a.id == b.id);
    v
}

/// Sort ids naturally: `REQ-2` before `REQ-10`.
fn id_key(id: &str) -> (String, u64, String) {
    match id.rsplit_once('-') {
        Some((stem, n)) if n.chars().all(|c| c.is_ascii_digit()) && !n.is_empty() => (
            stem.to_string(),
            n.parse().unwrap_or(u64::MAX),
            id.to_string(),
        ),
        _ => (id.to_string(), 0, id.to_string()),
    }
}

/// The lower-case anchor of an id.
pub fn anchor(id: &str) -> String {
    id.to_ascii_lowercase()
}

fn str_field<'a>(v: &'a toml::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(toml::Value::as_str)
}

fn list_field(v: &toml::Value, key: &str) -> Vec<String> {
    match v.get(key) {
        Some(toml::Value::Array(a)) => a
            .iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect(),
        Some(toml::Value::String(s)) => vec![s.clone()],
        _ => Vec::new(),
    }
}

/// Text for a Markdown table cell: one line, `|` escaped.
fn cell(text: &str) -> String {
    text.trim()
        .replace('\r', "")
        .replace('|', "\\|")
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("<br>")
}

/// Where each exported id lives, for links: id → (root, kind).
type Index = BTreeMap<String, Vec<(String, EntryKind)>>;

/// The link to `id` from a file `depth` folders below the export path,
/// preferring the same root. Plain text when the id is not exported.
fn link(index: &Index, id: &str, root: &str, depth: usize) -> String {
    let Some(places) = index.get(id) else {
        return id.to_string();
    };
    let (r, kind) = places
        .iter()
        .find(|(r, _)| r == root)
        .or_else(|| places.first())
        .cloned()
        .expect("non-empty");
    let up = "../".repeat(depth);
    let target = match kind {
        EntryKind::Decision => format!("{up}decisions/{r}/{id}.md"),
        EntryKind::Requirement => format!("{up}requirements/{r}.md#{}", anchor(id)),
        EntryKind::OpenQuestion => format!("{up}open_questions/{r}.md#{}", anchor(id)),
        EntryKind::Source => return id.to_string(),
    };
    format!("[{id}]({target})")
}

fn links(index: &Index, ids: &[String], root: &str, depth: usize) -> String {
    ids.iter()
        .map(|i| link(index, i, root, depth))
        .collect::<Vec<_>>()
        .join(", ")
}

fn kind_name(kind: EntryKind) -> &'static str {
    kind.table()
}

/// The export, file by file: path relative to the export folder → text.
pub fn render(layout: &LayoutContext, export: &ExportSection) -> Result<BTreeMap<PathBuf, String>> {
    let wanted: BTreeSet<&str> = export.entities.iter().map(String::as_str).collect();
    let omitted = |kind: EntryKind, field: &str| {
        (kind == EntryKind::Decision && NEVER_EXPORTED.contains(&field))
            || export
                .omit
                .get(kind_name(kind))
                .is_some_and(|f| f.iter().any(|x| x == field))
    };
    let kinds = [
        EntryKind::Decision,
        EntryKind::Requirement,
        EntryKind::OpenQuestion,
    ];

    // Load every root once.
    let mut loaded: Vec<(String, BTreeMap<EntryKind, Vec<Item>>)> = Vec::new();
    for root in roots(layout)? {
        let mut by_kind = BTreeMap::new();
        for kind in kinds {
            if wanted.contains(kind_name(kind)) {
                by_kind.insert(kind, items(root.store.load(kind)?));
            }
        }
        loaded.push((root.name, by_kind));
    }
    let mut index: Index = BTreeMap::new();
    for (root, by_kind) in &loaded {
        for (kind, list) in by_kind {
            for it in list {
                index
                    .entry(it.id.clone())
                    .or_default()
                    .push((root.clone(), *kind));
            }
        }
    }

    let mut files = BTreeMap::new();
    for (root, by_kind) in &loaded {
        if let Some(decisions) = by_kind.get(&EntryKind::Decision) {
            // Reverse links: who supersedes a decision; which requirements
            // name it in `decided_by`.
            let mut superseded_by: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for d in decisions {
                for old in list_field(&d.value, "supersedes") {
                    superseded_by.entry(old).or_default().push(d.id.clone());
                }
            }
            let mut decided: BTreeMap<String, Vec<String>> = BTreeMap::new();
            if let Some(reqs) = by_kind.get(&EntryKind::Requirement) {
                for r in reqs {
                    for d in list_field(&r.value, "decided_by") {
                        decided.entry(d).or_default().push(r.id.clone());
                    }
                }
            }
            for d in decisions {
                files.insert(
                    PathBuf::from("decisions")
                        .join(root)
                        .join(format!("{}.md", d.id)),
                    decision_md(
                        d,
                        root,
                        &index,
                        &omitted,
                        superseded_by.get(&d.id),
                        decided.get(&d.id),
                    ),
                );
            }
        }
        if let Some(reqs) = by_kind.get(&EntryKind::Requirement) {
            files.insert(
                PathBuf::from("requirements").join(format!("{root}.md")),
                requirements_md(reqs, root, &index, &omitted),
            );
        }
        if let Some(oqs) = by_kind.get(&EntryKind::OpenQuestion) {
            files.insert(
                PathBuf::from("open_questions").join(format!("{root}.md")),
                open_questions_md(oqs, root, &index, &omitted),
            );
        }
    }
    Ok(files)
}

/// Decision fields shown in the header or as links, not as sections.
const DECISION_HEADER: [&str; 7] = [
    "id",
    "title",
    "status",
    "supersedes",
    "closes_questions",
    "defaults_for",
    "record",
];

/// The order of a decision's prose sections; other fields follow, sorted.
const DECISION_ORDER: [&str; 7] = [
    "text",
    "context",
    "consequences",
    "alternatives",
    "verification",
    "source",
    "source_ref",
];

fn decision_md(
    d: &Item,
    root: &str,
    index: &Index,
    omitted: &dyn Fn(EntryKind, &str) -> bool,
    superseded_by: Option<&Vec<String>>,
    decided: Option<&Vec<String>>,
) -> String {
    let k = EntryKind::Decision;
    let mut out = String::new();
    let title = str_field(&d.value, "title").unwrap_or("");
    out.push_str(&format!(
        "<a id=\"{}\"></a>\n\n# {} — {}\n\n",
        anchor(&d.id),
        d.id,
        title.trim()
    ));
    if let Some(status) = str_field(&d.value, "status").filter(|_| !omitted(k, "status")) {
        out.push_str(&format!("**Status:** {status}\n\n"));
    }
    for (field, label) in [
        ("supersedes", "Supersedes"),
        ("closes_questions", "Closes"),
        ("defaults_for", "Default for"),
    ] {
        let ids = list_field(&d.value, field);
        if !ids.is_empty() && !omitted(k, field) {
            out.push_str(&format!("**{label}:** {}\n\n", links(index, &ids, root, 2)));
        }
    }
    if let Some(by) = superseded_by {
        out.push_str(&format!(
            "**Superseded by:** {}\n\n",
            links(index, by, root, 2)
        ));
    }
    if let Some(reqs) = decided {
        let mut reqs = reqs.clone();
        reqs.sort_by_key(|a| id_key(a));
        out.push_str(&format!(
            "**Requirements:** {}\n\n",
            links(index, &reqs, root, 2)
        ));
    }
    let table = d.value.as_table().cloned().unwrap_or_default();
    let mut fields: Vec<&str> = DECISION_ORDER
        .iter()
        .copied()
        .filter(|f| table.contains_key(*f))
        .collect();
    let mut rest: Vec<&str> = table
        .keys()
        .map(String::as_str)
        .filter(|f| !DECISION_HEADER.contains(f) && !DECISION_ORDER.contains(f))
        .collect();
    rest.sort();
    fields.extend(rest);
    for field in fields {
        if omitted(k, field) {
            continue;
        }
        let body = match table.get(field) {
            Some(toml::Value::String(s)) => s.trim().to_string(),
            Some(toml::Value::Array(a)) => a
                .iter()
                .map(|x| match x.as_str() {
                    Some(s) => link(index, s, root, 2),
                    None => x.to_string(),
                })
                .collect::<Vec<_>>()
                .join(", "),
            Some(other) => other.to_string(),
            None => continue,
        };
        if body.is_empty() {
            continue;
        }
        if field == "text" {
            out.push_str(&format!("{body}\n\n"));
        } else {
            out.push_str(&format!("## {}\n\n{body}\n\n", heading(field)));
        }
    }
    format!("{}\n", out.trim_end())
}

/// `source_ref` → `Source ref`.
fn heading(field: &str) -> String {
    let words = field.replace('_', " ");
    let mut c = words.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn requirements_md(
    reqs: &[Item],
    root: &str,
    index: &Index,
    omitted: &dyn Fn(EntryKind, &str) -> bool,
) -> String {
    let k = EntryKind::Requirement;
    let show = |f: &str| !omitted(k, f);
    let mut cols = vec!["Id", "Requirement"];
    if show("satisfies") {
        cols.push("Part of");
    }
    if show("source") || show("source_ref") {
        cols.push("Source");
    }
    if show("verified_by") {
        cols.push("Verified by");
    }
    if show("decided_by") {
        cols.push("Decided by");
    }
    let mut out = format!(
        "# Requirements — {root}\n\n| {} |\n|{}\n",
        cols.join(" | "),
        "---|".repeat(cols.len())
    );
    for r in reqs {
        let mut text = cell(str_field(&r.value, "text").unwrap_or(""));
        let retired = list_field(&r.value, "retired_by");
        if !retired.is_empty() {
            text = format!("*Retired, see {}.* {text}", links(index, &retired, root, 1));
        }
        let mut row = vec![format!("<a id=\"{}\"></a>{}", anchor(&r.id), r.id), text];
        if show("satisfies") {
            row.push(links(index, &list_field(&r.value, "satisfies"), root, 1));
        }
        if show("source") || show("source_ref") {
            let mut parts = Vec::new();
            if show("source") {
                parts.extend(list_field(&r.value, "source"));
            }
            if show("source_ref") {
                if let Some(s) = str_field(&r.value, "source_ref") {
                    parts.push(cell(s));
                }
            }
            row.push(parts.join("; "));
        }
        if show("verified_by") {
            let tests = list_field(&r.value, "verified_by");
            let pending = r.value.get("test_pending").and_then(toml::Value::as_bool) == Some(true);
            row.push(if tests.is_empty() && pending {
                "*pending*".to_string()
            } else {
                tests
                    .iter()
                    .map(|t| format!("`{}`", cell(t).replace('`', "'")))
                    .collect::<Vec<_>>()
                    .join(", ")
            });
        }
        if show("decided_by") {
            row.push(links(index, &list_field(&r.value, "decided_by"), root, 1));
        }
        out.push_str(&format!("| {} |\n", row.join(" | ")));
    }
    out
}

fn open_questions_md(
    oqs: &[Item],
    root: &str,
    index: &Index,
    omitted: &dyn Fn(EntryKind, &str) -> bool,
) -> String {
    let k = EntryKind::OpenQuestion;
    let show = |f: &str| !omitted(k, f);
    let mut out = format!("# Open questions — {root}\n\n| Id | Question | Status | Affects | Resolved by |\n|---|---|---|---|---|\n");
    for q in oqs {
        let mut text = String::new();
        if let Some(t) = str_field(&q.value, "title").filter(|_| show("title")) {
            text.push_str(&format!("**{}**", cell(t)));
        }
        if let Some(t) = str_field(&q.value, "text").filter(|_| show("text")) {
            if !text.is_empty() {
                text.push_str("<br>");
            }
            text.push_str(&cell(t));
        }
        let status = str_field(&q.value, "status")
            .filter(|_| show("status"))
            .unwrap_or("");
        let affects = if show("affects") {
            links(index, &list_field(&q.value, "affects"), root, 1)
        } else {
            String::new()
        };
        let mut by = Vec::new();
        for f in ["closed_by", "default_via"] {
            if show(f) {
                by.extend(list_field(&q.value, f));
            }
        }
        out.push_str(&format!(
            "| <a id=\"{}\"></a>{} | {text} | {status} | {affects} | {} |\n",
            anchor(&q.id),
            q.id,
            links(index, &by, root, 1)
        ));
    }
    out
}

/// What `export` wrote.
pub struct Written {
    pub dir: PathBuf,
    pub written: usize,
    pub removed: usize,
}

/// Every `.md` file under the owned folders of `dir`, relative to it.
pub fn existing(dir: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    for sub in OWNED_DIRS {
        let mut files = BTreeSet::new();
        crate::import_edges::walk(&dir.join(sub), &mut files);
        for f in files {
            if f.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Ok(rel) = f.strip_prefix(dir) {
                    out.insert(rel.to_path_buf());
                }
            }
        }
    }
    out
}

/// Write the export: every rendered file, and remove owned files the
/// spec no longer produces.
pub fn write(layout: &LayoutContext) -> Result<Written> {
    let Some(export) = &layout.spec.export else {
        bail!("[EXPORT-003] spec.toml has no `[export]` table: add `[export] path = \"docs/spec\"` (relative to the code root)");
    };
    let dir = export_dir(layout, export)?;
    let files = render(layout, export)?;
    let mut removed = 0;
    for old in existing(&dir) {
        if !files.contains_key(&old) {
            std::fs::remove_file(dir.join(&old))
                .with_context(|| format!("removing {}", dir.join(&old).display()))?;
            removed += 1;
        }
    }
    for (rel, text) in &files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(Written {
        dir,
        written: files.len(),
        removed,
    })
}

/// How the files on disk differ from what `export` writes now: missing,
/// different and extra files, relative to the export folder.
pub fn differences(
    layout: &LayoutContext,
    export: &ExportSection,
) -> Result<Vec<(PathBuf, &'static str)>> {
    let dir = export_dir(layout, export)?;
    let files = render(layout, export)?;
    let mut out = Vec::new();
    for (rel, text) in &files {
        match std::fs::read_to_string(dir.join(rel)) {
            Err(_) => out.push((rel.clone(), "missing")),
            Ok(have) if have != *text => out.push((rel.clone(), "differs")),
            Ok(_) => {}
        }
    }
    for old in existing(&dir) {
        if !files.contains_key(&old) {
            out.push((old, "not produced by the spec"));
        }
    }
    Ok(out)
}
