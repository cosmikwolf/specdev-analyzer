//! The 2.0 field cuts: what `migrate` removes from a project's spec files,
//! and the report line for every removed value.
//!
//! A field leaves when it copied a file that is itself the truth (the
//! project's manifest) or the code's own syntax, and nothing reads it any
//! more. Each removed value is reported with the comments attached to it and
//! a mark that says whether the manifest holds the same value:
//!
//! - `spec-only` — the package has no manifest;
//! - `in manifest` / `differs: <keys>` / `not in manifest` — a dependency
//!   entry or a package field, compared with the manifest;
//! - `prose` — free text (a description) that leaves with its entry;
//! - `not compared` — anything else.

use toml_edit::{DocumentMut, Item, Table};

use crate::manifest_facts::{ManifestDep, ManifestFacts};

/// Architecture keys that leave, as dotted paths. A whole table is named by
/// its key.
pub const ARCHITECTURE_REMOVED: &[&str] = &[
    "external_deps",
    "features",
    "profiles",
    "test_targets",
    "workspace_dependencies",
    "dependency_overrides",
    "workspace_package",
    "msrv",
    "version",
    "environment",
    "scripts",
    "flutter.assets",
    "flutter.uses_material_design",
    "plugin.version",
    "platform.mcu_core",
    "platform.mcu_runtime",
    "platform.mcu_part",
    "runtime.allocator",
    "runtime.critical_section_impl",
    "runtime.realtime_audit",
    "logger",
];

/// Paths whose values are dependency entries (`crate` / `package` +
/// `version` …), compared with the manifest's dependencies.
const DEPENDENCY_PATHS: &[&str] = &[
    "external_deps",
    "workspace_dependencies",
    "platform.mcu_core",
    "platform.mcu_runtime",
    "runtime.panic_handler",
    "runtime.allocator",
    "runtime.critical_section_impl",
    "logger.provider",
    "logger.backend",
];

/// Paths whose values are package fields, compared with the manifest's
/// package table: spec path → manifest field.
const PACKAGE_PATHS: &[(&str, &str)] = &[
    ("version", "version"),
    ("plugin.version", "version"),
    ("msrv", "rust-version"),
    ("workspace_package.version", "version"),
    ("workspace_package.edition", "edition"),
    ("workspace_package.license", "license"),
    ("workspace_package.repository", "repository"),
    ("workspace_package.rust-version", "rust-version"),
    ("[project].version", "version"),
    ("[project].description", "description"),
    ("[project].license", "license"),
    ("[project].edition", "edition"),
    ("[project].repository", "repository"),
];

/// One removed value, before it is marked.
#[derive(Debug, Clone)]
pub struct Removed {
    /// Dotted key path (`external_deps`, `platform.mcu_core`, …).
    pub key: String,
    /// The value, rendered on one line.
    pub value: String,
    /// Comment lines attached to it, as written.
    pub comments: Vec<String>,
    /// The value as TOML, for the comparison.
    pub raw: Option<toml::Value>,
}

/// One report line.
#[derive(Debug, Clone)]
pub struct ReportLine {
    pub file: String,
    pub removed: Removed,
    pub mark: String,
}

impl ReportLine {
    /// The lines `migrate` prints.
    pub fn render(&self, verb: &str) -> String {
        let mut s = format!("{verb}remove from {}: {} = {}  [{}]", self.file, self.removed.key, self.removed.value, self.mark);
        for c in &self.removed.comments {
            s.push_str("\n      ");
            s.push_str(c);
        }
        s
    }
}

fn comments_of(decor: Option<&str>) -> Vec<String> {
    decor
        .map(|d| d.lines().map(str::trim).filter(|l| l.starts_with('#')).map(str::to_string).collect())
        .unwrap_or_default()
}

fn one_line(item: &Item) -> String {
    match item {
        Item::Value(v) => v.to_string().trim().to_string(),
        Item::Table(t) => {
            let mut inline = toml_edit::InlineTable::new();
            for (k, v) in t.iter() {
                if let Some(v) = v.as_value() {
                    inline.insert(k, v.clone());
                }
            }
            inline.fmt();
            inline.to_string().trim().trim_start_matches('{').trim_end_matches('}').trim().to_string()
        }
        Item::ArrayOfTables(a) => format!("{} entries", a.len()),
        Item::None => String::new(),
    }
}

fn to_toml(item: &Item) -> Option<toml::Value> {
    let text = match item {
        Item::Value(v) => format!("x = {}", v.to_string().trim()),
        Item::Table(t) => format!("[x]\n{t}"),
        _ => return None,
    };
    let doc: toml::Table = text.parse().ok()?;
    doc.get("x").cloned()
}

/// Remove `path` from `table`, collecting what goes. An array of tables
/// gives one [`Removed`] per element (each with its own comments).
fn take(table: &mut Table, path: &str, out: &mut Vec<Removed>) {
    let mut parts = path.splitn(2, '.');
    let head = parts.next().unwrap_or("");
    let rest = parts.next();
    match rest {
        Some(rest) => {
            let start = out.len();
            let now_empty = match table.get_mut(head) {
                Some(Item::Table(sub)) => {
                    take(sub, rest, out);
                    sub.is_empty()
                }
                _ => return,
            };
            for x in &mut out[start..] {
                x.key = format!("{head}.{}", x.key);
            }
            if now_empty {
                // The emptied table's own header comment leaves with it.
                if let Some(Item::Table(sub)) = table.remove(head) {
                    let c = comments_of(sub.decor().prefix().and_then(|p| p.as_str()));
                    if let Some(first) = out.get_mut(start) {
                        first.comments.splice(0..0, c);
                    }
                }
            }
        }
        None => {
            let Some(item) = table.remove(head) else { return };
            let key = head.to_string();
            match &item {
                Item::ArrayOfTables(a) => {
                    for t in a.iter() {
                        let it = Item::Table(t.clone());
                        out.push(Removed {
                            key: key.clone(),
                            value: one_line(&it),
                            comments: comments_of(t.decor().prefix().and_then(|p| p.as_str())),
                            raw: to_toml(&it),
                        });
                    }
                }
                Item::Table(t) => {
                    // A sub-table of tables (`[logger.provider]` …): one line each.
                    let nested: Vec<(String, Item)> =
                        t.iter().filter(|(_, v)| v.is_table()).map(|(k, v)| (k.to_string(), v.clone())).collect();
                    if !nested.is_empty() && nested.len() == t.len() {
                        for (k, v) in nested {
                            let comments = v.as_table().and_then(|x| x.decor().prefix()).and_then(|p| p.as_str());
                            out.push(Removed {
                                key: format!("{key}.{k}"),
                                value: one_line(&v),
                                comments: comments_of(comments),
                                raw: to_toml(&v),
                            });
                        }
                    } else {
                        out.push(Removed {
                            key: key.clone(),
                            value: one_line(&item),
                            comments: comments_of(t.decor().prefix().and_then(|p| p.as_str())),
                            raw: to_toml(&item),
                        });
                    }
                }
                Item::Value(v) => out.push(Removed {
                    key,
                    value: one_line(&item),
                    comments: comments_of(v.decor().prefix().and_then(|p| p.as_str()))
                        .into_iter()
                        .chain(comments_of(v.decor().suffix().and_then(|p| p.as_str())))
                        .collect(),
                    raw: to_toml(&item),
                }),
                Item::None => {}
            }
        }
    }
}

/// `architecture.toml` without the removed keys. Returns the new text, what
/// was removed, and whether any key is left.
pub fn strip_architecture(text: &str) -> anyhow::Result<(String, Vec<Removed>, bool)> {
    let mut doc: DocumentMut = text.parse()?;
    let mut removed = Vec::new();
    for path in ARCHITECTURE_REMOVED {
        take(doc.as_table_mut(), path, &mut removed);
    }
    // A firmware panic handler is a dependency entry (a table); an audio
    // plugin's is a strategy string and stays.
    if doc.get("runtime").and_then(|r| r.get("panic_handler")).is_some_and(|p| p.is_table()) {
        take(doc.as_table_mut(), "runtime.panic_handler", &mut removed);
    }
    if doc.get("runtime").and_then(|r| r.as_table()).is_some_and(|r| r.is_empty()) {
        doc.remove("runtime");
    }
    let keys_left = !doc.as_table().is_empty();
    Ok((doc.to_string(), removed, keys_left))
}

/// A module spec without the removed keys.
pub fn strip_module(doc: &mut DocumentMut) -> Vec<Removed> {
    let mut removed = Vec::new();
    take(doc.as_table_mut(), "external_deps", &mut removed);
    take(doc.as_table_mut(), "feature_gates", &mut removed);
    let mut nested = |key: &str, field: &str, sub: Option<&str>, removed: &mut Vec<Removed>| {
        if let Some(arr) = doc.get_mut(key).and_then(Item::as_array_of_tables_mut) {
            for t in arr.iter_mut() {
                match sub {
                    None => take(t, field, removed),
                    Some(inner) => {
                        if let Some(params) = t.get_mut(field).and_then(Item::as_array_mut) {
                            for p in params.iter_mut() {
                                if let Some(it) = p.as_inline_table_mut() {
                                    if let Some(v) = it.remove(inner) {
                                        removed.push(Removed {
                                            key: format!("{key}[].{field}[].{inner}"),
                                            value: v.to_string().trim().to_string(),
                                            comments: Vec::new(),
                                            raw: None,
                                        });
                                    }
                                }
                            }
                        } else if let Some(params) = t.get_mut(field).and_then(Item::as_array_of_tables_mut) {
                            for p in params.iter_mut() {
                                take(p, inner, removed);
                            }
                        }
                    }
                }
            }
        }
    };
    nested("actions", "returns", None, &mut removed);
    nested("actions", "params", Some("type"), &mut removed);
    nested("errors", "variant", None, &mut removed);
    removed
}

fn dep_name(v: &toml::Value) -> Option<String> {
    ["crate", "package", "name"].iter().find_map(|k| v.get(*k).and_then(|x| x.as_str()).map(str::to_string))
}

fn compare_dep(spec: &toml::Value, m: &ManifestDep) -> Vec<String> {
    let mut diff = Vec::new();
    if let (Some(a), Some(b)) = (spec.get("version").and_then(|v| v.as_str()), m.version.as_deref()) {
        if a != b {
            diff.push(format!("version {a} vs {b}"));
        }
    }
    if let Some(f) = spec.get("features").and_then(|v| v.as_array()) {
        let mut a: Vec<String> = f.iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
        // A `source = "workspace"` entry states the member's own additions.
        let workspace = spec.get("source").and_then(|v| v.as_str()) == Some("workspace");
        let mut b = if workspace { m.own_features.clone() } else { m.features.clone() };
        a.sort();
        b.sort();
        if a != b {
            diff.push(format!("features {a:?} vs {b:?}"));
        }
    }
    if let Some(d) = spec.get("default_features").and_then(|v| v.as_bool()) {
        if m.default_features.unwrap_or(true) != d {
            diff.push("default_features".into());
        }
    }
    if let Some(o) = spec.get("optional").and_then(|v| v.as_bool()) {
        if o != m.optional {
            diff.push("optional".into());
        }
    }
    if let Some(t) = spec.get("test_only").and_then(|v| v.as_bool()) {
        if t != m.dev {
            diff.push("test_only".into());
        }
    }
    if let Some(src) = spec.get("source").and_then(|v| v.as_str()) {
        let spec_path = spec.get("path").and_then(|v| v.as_str());
        match (src, &m.source) {
            ("path", Some(ms)) if ms.starts_with("path:") => {
                if spec_path.is_some_and(|p| ms != &format!("path:{p}")) {
                    diff.push("source".into());
                }
            }
            ("path", _) => diff.push("source".into()),
            ("git", Some(ms)) if ms.starts_with("git:") => {}
            ("git", _) => diff.push("source".into()),
            _ => {}
        }
    }
    diff
}

/// Mark one removed value against the manifest (`None`: the package has no
/// manifest).
pub fn mark(removed: &Removed, facts: Option<&ManifestFacts>) -> String {
    let Some(facts) = facts else { return "spec-only".into() };
    let path = removed.key.as_str();
    if DEPENDENCY_PATHS.contains(&path) {
        if let Some(raw) = &removed.raw {
            if let Some(name) = dep_name(raw) {
                return match facts.deps.get(&name) {
                    None => "not in manifest".into(),
                    Some(entries) => {
                        // The entry in the same table (dev or not) is the one
                        // to compare with; any exact match is a match.
                        let diffs: Vec<Vec<String>> = entries.iter().map(|m| compare_dep(raw, m)).collect();
                        if diffs.iter().any(Vec::is_empty) {
                            "in manifest".into()
                        } else {
                            let want_dev = raw.get("test_only").and_then(|v| v.as_bool()).unwrap_or(false);
                            let best = entries.iter().position(|m| m.dev == want_dev).unwrap_or(0);
                            format!("differs: {}", diffs[best].join(", "))
                        }
                    }
                };
            }
        }
    }
    if let Some((_, field)) = PACKAGE_PATHS.iter().find(|(p, _)| *p == path) {
        let spec_value = removed.raw.as_ref().and_then(|v| v.as_str().map(str::to_string));
        return match (spec_value, facts.package.get(*field)) {
            (_, None) => "not in manifest".into(),
            (Some(a), Some(b)) if &a == b => "in manifest".into(),
            (Some(a), Some(b)) => format!("differs: {a} vs {b}"),
            (None, Some(_)) => "not compared".into(),
        };
    }
    if removed.raw.as_ref().is_some_and(|v| v.get("description").is_some()) || path.ends_with("description") {
        return "prose".into();
    }
    "not compared".into()
}

/// Reduction kinds and relations that existed only to keep a removed copy
/// consistent with itself.
pub const REMOVED_KINDS: &[&str] =
    &["Feature", "feature_depends_on", "module_feature_gate", "WorkspaceDependency", "workspace_default_member", "Workspace", "test_verifies"];

/// A reduction file without the entries of [`REMOVED_KINDS`] (each `- kind:`
/// list item, with the comment block directly above it). Returns the text
/// and the kinds removed.
pub fn drop_reduction_kinds(text: &str) -> (String, Vec<String>) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out: Vec<&str> = Vec::new();
    let mut removed = Vec::new();
    let mut i = 0;
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    while i < lines.len() {
        let l = lines[i];
        let t = l.trim();
        if let Some(kind) = t.strip_prefix("- kind:").map(str::trim) {
            if REMOVED_KINDS.contains(&kind) {
                let ind = indent_of(l);
                removed.push(kind.to_string());
                while out.last().is_some_and(|p| p.trim().starts_with('#') && indent_of(p) == ind) {
                    out.pop();
                }
                i += 1;
                while i < lines.len()
                    && (lines[i].trim().is_empty() || indent_of(lines[i]) > ind)
                    && !lines[i].trim().starts_with("- kind:")
                {
                    i += 1;
                }
                continue;
            }
        }
        out.push(l);
        i += 1;
    }
    (out.concat(), removed)
}
