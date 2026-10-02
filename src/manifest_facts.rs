//! What a project's own manifest says: its dependencies and its package
//! fields, read only to tell the user, when `migrate` removes a spec copy of
//! one, whether the manifest holds the same value.
//!
//! Each language adapter names its manifest and calls the reader here
//! ([`crate::lang::LanguageAdapter::manifest_facts`]). Nothing here writes a
//! manifest, and no check reads these facts.

use std::collections::BTreeMap;
use std::path::Path;

/// One dependency as a manifest declares it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestDep {
    pub version: Option<String>,
    /// Features after `workspace = true` is resolved.
    pub features: Vec<String>,
    /// The features the entry itself lists (a member's additions).
    pub own_features: Vec<String>,
    pub default_features: Option<bool>,
    pub optional: bool,
    /// `path:<p>`, `git:<url>`, `sdk:<name>`, or `None` for a registry.
    pub source: Option<String>,
    /// A dev / test dependency.
    pub dev: bool,
}

/// A manifest's facts.
#[derive(Debug, Clone, Default)]
pub struct ManifestFacts {
    /// Keyed by the real package name (a `package =` rename resolved); a
    /// crate listed in several tables (normal and dev) has one entry each.
    pub deps: BTreeMap<String, Vec<ManifestDep>>,
    /// Package fields by spec name (`version`, `description`, `license`,
    /// `edition`, `repository`, `rust-version`).
    pub package: BTreeMap<String, String>,
}

fn s(v: &toml::Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

/// `Cargo.toml` at `root`, with `workspace = true` entries resolved against
/// the Cargo workspace root (the nearest ancestor `Cargo.toml` with a
/// `[workspace]` table).
pub fn cargo(root: &Path) -> Option<ManifestFacts> {
    let doc: toml::Table = std::fs::read_to_string(root.join("Cargo.toml")).ok()?.parse().ok()?;
    let ws = cargo_workspace(root, &doc);
    let ws_deps = ws.as_ref().and_then(|w| w.get("dependencies")).and_then(|d| d.as_table()).cloned().unwrap_or_default();
    let ws_pkg = ws.as_ref().and_then(|w| w.get("package")).and_then(|d| d.as_table()).cloned().unwrap_or_default();
    let mut facts = ManifestFacts::default();
    let mut tables: Vec<(&toml::Table, bool)> = Vec::new();
    for (key, dev) in [("dependencies", false), ("dev-dependencies", true), ("build-dependencies", false)] {
        if let Some(t) = doc.get(key).and_then(|v| v.as_table()) {
            tables.push((t, dev));
        }
    }
    if let Some(targets) = doc.get("target").and_then(|v| v.as_table()) {
        for t in targets.values().filter_map(|v| v.as_table()) {
            for (key, dev) in [("dependencies", false), ("dev-dependencies", true), ("build-dependencies", false)] {
                if let Some(d) = t.get(key).and_then(|v| v.as_table()) {
                    tables.push((d, dev));
                }
            }
        }
    }
    // A workspace root lists its own shared dependencies too.
    if let Some(d) = doc.get("workspace").and_then(|w| w.get("dependencies")).and_then(|v| v.as_table()) {
        tables.push((d, false));
    }
    for (table, dev) in tables {
        for (key, v) in table {
            let (name, dep) = cargo_dep(key, v, &ws_deps, dev);
            facts.deps.entry(name).or_default().push(dep);
        }
    }
    let pkg = doc.get("package").and_then(|v| v.as_table()).cloned().unwrap_or_default();
    for field in ["version", "description", "license", "edition", "repository", "rust-version"] {
        let v = match pkg.get(field) {
            Some(toml::Value::Table(t)) if t.get("workspace").and_then(|w| w.as_bool()) == Some(true) => {
                ws_pkg.get(field).and_then(s)
            }
            Some(v) => s(v),
            None => None,
        };
        if let Some(v) = v {
            facts.package.insert(field.to_string(), v);
        }
    }
    if facts.package.is_empty() {
        for (k, v) in &ws_pkg {
            if let Some(v) = s(v) {
                facts.package.insert(k.clone(), v);
            }
        }
    }
    Some(facts)
}

fn cargo_workspace(root: &Path, doc: &toml::Table) -> Option<toml::Table> {
    if let Some(w) = doc.get("workspace").and_then(|v| v.as_table()) {
        return Some(w.clone());
    }
    let mut cur = root.parent();
    while let Some(dir) = cur {
        if let Some(t) = std::fs::read_to_string(dir.join("Cargo.toml")).ok().and_then(|x| x.parse::<toml::Table>().ok()) {
            if let Some(w) = t.get("workspace").and_then(|v| v.as_table()) {
                return Some(w.clone());
            }
        }
        cur = dir.parent();
    }
    None
}

fn cargo_dep(key: &str, v: &toml::Value, ws: &toml::Table, dev: bool) -> (String, ManifestDep) {
    let mut dep = ManifestDep { dev, ..Default::default() };
    let mut name = key.to_string();
    match v {
        toml::Value::String(version) => dep.version = Some(version.clone()),
        toml::Value::Table(t) => {
            dep.own_features =
                t.get("features").and_then(|f| f.as_array()).map(|a| a.iter().filter_map(s).collect()).unwrap_or_default();
            let mut merged = t.clone();
            if t.get("workspace").and_then(|w| w.as_bool()) == Some(true) {
                match ws.get(key) {
                    Some(toml::Value::String(version)) => {
                        merged.insert("version".into(), toml::Value::String(version.clone()));
                    }
                    Some(toml::Value::Table(base)) => {
                        let mut b = base.clone();
                        // The member's features add to the workspace's.
                        let mut feats: Vec<toml::Value> = b.get("features").and_then(|f| f.as_array()).cloned().unwrap_or_default();
                        feats.extend(t.get("features").and_then(|f| f.as_array()).cloned().unwrap_or_default());
                        for (k, v) in t {
                            if k != "workspace" && k != "features" {
                                b.insert(k.clone(), v.clone());
                            }
                        }
                        if !feats.is_empty() {
                            b.insert("features".into(), toml::Value::Array(feats));
                        }
                        merged = b;
                    }
                    _ => {}
                }
            }
            if let Some(p) = merged.get("package").and_then(s) {
                name = p;
            }
            dep.version = merged.get("version").and_then(s);
            dep.features = merged
                .get("features")
                .and_then(|f| f.as_array())
                .map(|a| a.iter().filter_map(s).collect())
                .unwrap_or_default();
            dep.default_features = merged.get("default-features").or(merged.get("default_features")).and_then(|v| v.as_bool());
            dep.optional = merged.get("optional").and_then(|v| v.as_bool()).unwrap_or(false);
            dep.source = merged
                .get("path")
                .and_then(s)
                .map(|p| format!("path:{p}"))
                .or_else(|| merged.get("git").and_then(s).map(|g| format!("git:{g}")));
        }
        _ => {}
    }
    (name, dep)
}

/// `package.json` at `root`.
pub fn npm(root: &Path) -> Option<ManifestFacts> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(root.join("package.json")).ok()?).ok()?;
    let mut facts = ManifestFacts::default();
    for (key, dev) in [("dependencies", false), ("devDependencies", true), ("peerDependencies", false), ("optionalDependencies", false)] {
        if let Some(m) = v.get(key).and_then(|d| d.as_object()) {
            for (name, ver) in m {
                let version = ver.as_str().map(str::to_string);
                let source = version.as_deref().and_then(|x| {
                    ["link:", "file:", "workspace:", "git+", "github:"].iter().any(|p| x.starts_with(p)).then(|| x.to_string())
                });
                facts.deps.entry(name.clone()).or_default().push(ManifestDep { version, source, dev, ..Default::default() });
            }
        }
    }
    for field in ["version", "description", "license", "repository"] {
        if let Some(x) = v.get(field).and_then(|x| x.as_str()) {
            facts.package.insert(field.to_string(), x.to_string());
        }
    }
    Some(facts)
}

/// `pubspec.yaml` at `root`.
pub fn pub_(root: &Path) -> Option<ManifestFacts> {
    let v: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(root.join("pubspec.yaml")).ok()?).ok()?;
    let mut facts = ManifestFacts::default();
    for (key, dev) in [("dependencies", false), ("dev_dependencies", true), ("dependency_overrides", false)] {
        if let Some(m) = v.get(key).and_then(|d| d.as_mapping()) {
            for (name, spec) in m {
                let Some(name) = name.as_str() else { continue };
                let mut dep = ManifestDep { dev, ..Default::default() };
                match spec {
                    serde_yaml::Value::String(x) => dep.version = Some(x.clone()),
                    serde_yaml::Value::Mapping(m) => {
                        dep.version = m.get("version").and_then(|x| x.as_str()).map(str::to_string);
                        dep.source = ["sdk", "path", "git", "hosted"].iter().find_map(|k| {
                            m.get(*k).map(|x| format!("{k}:{}", x.as_str().map(str::to_string).unwrap_or_else(|| "…".into())))
                        });
                    }
                    _ => {}
                }
                facts.deps.entry(name.to_string()).or_default().push(dep);
            }
        }
    }
    for field in ["version", "description", "repository"] {
        if let Some(x) = v.get(field).and_then(|x| x.as_str()) {
            facts.package.insert(field.to_string(), x.to_string());
        }
    }
    Some(facts)
}
