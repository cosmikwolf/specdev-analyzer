//! Extract pipeline — emit canonical YAML from validated TOML specs.
//!
//! Produces:
//!
//! * `<out>/<ModuleName>.yaml` — one per module, passthrough of the spec
//!   with keys canonicalized.
//! * `<out>/architecture.yaml` — synthesized summary with a
//!   dependency_graph and per-module (name, layer) pairs, plus the raw
//!   connections from `specs/architecture.toml` if present.
//! * `<out>/requirements.yaml` — flattened list of requirements.
//!
//! Output is deterministic: every map key is re-ordered alphabetically
//! before serialization so two runs on identical input produce
//! byte-identical files.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::config::SpecConfig;
use crate::domain::DomainConfig;
use crate::packages;
use crate::validate::{self, Inputs};

pub struct ExtractionResult {
    /// (module_name, module value) pairs, sorted by qualified name
    /// (`<pkg>::<name>` when packaged, otherwise just `<name>`).
    pub modules: Vec<ExtractedModule>,
    pub architecture: Value,
    pub requirements: Value,
    /// Domain-declared extra collections (see `[[collections]]` in
    /// `domain.toml`). Map of collection name → (item_name, value) pairs
    /// sorted by item name. Empty when the domain declares no collections.
    pub collections: BTreeMap<String, Vec<(String, Value)>>,
}

pub struct ExtractedModule {
    /// Bare module name (`name` field).
    pub name: String,
    /// Package, if any.
    pub package: Option<String>,
    /// Canonicalized module value. Gets a top-level `package` key injected
    /// when `package` is `Some`.
    pub value: Value,
}

/// Run the extract pipeline. Returns an error if validation fails.
pub fn run(inputs: &Inputs) -> Result<ExtractionResult> {
    let report = validate::run(inputs)?;
    anyhow::ensure!(
        !report.has_errors(),
        "refusing to extract: {} error(s) reported by validate",
        report.counts().0
    );

    let specs_dir = inputs.project_root.join("specs");
    let spec_cfg = SpecConfig::load(&inputs.spec_file)?;
    let declared: HashSet<&str> = spec_cfg.packages.iter().map(|p| p.name.as_str()).collect();

    // Modules.
    let mut modules: Vec<ExtractedModule> = Vec::new();
    let modules_dir = specs_dir.join("modules");
    let (module_files, _depth) = packages::collect_module_files(&modules_dir, &inputs.project_root);
    for mf in &module_files {
        let value = canonicalize(load_toml(&mf.path)?);
        let name = value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| mf.path.file_stem().and_then(|s| s.to_str()).unwrap_or("?"))
            .to_string();
        let package = match &mf.package {
            Some(p) if declared.contains(p.as_str()) => Some(p.clone()),
            _ => None,
        };
        let mut value = value;
        if let Some(pkg) = &package {
            if let Some(obj) = value.as_object_mut() {
                obj.insert("package".to_string(), Value::String(pkg.clone()));
                // Re-canonicalize to keep alphabetical key order after injection.
            }
            value = canonicalize(value);
        }
        modules.push(ExtractedModule {
            name,
            package,
            value,
        });
    }
    modules.sort_by(|a, b| {
        packages::qualify(a.package.as_deref(), &a.name)
            .cmp(&packages::qualify(b.package.as_deref(), &b.name))
    });

    // Architecture summary (synthesized).
    // Module names in the dependency_graph and summaries use qualified form
    // (`<pkg>::<name>`) whenever the project declares packages; flat
    // projects keep bare names so existing single-package snapshots remain
    // byte-stable.
    let project_has_packages = !spec_cfg.packages.is_empty();
    let qualify_for_graph = |pkg: Option<&str>, name: &str| -> String {
        if project_has_packages {
            packages::qualify(pkg, name)
        } else {
            name.to_string()
        }
    };

    // name → package lookup for rewriting depends_on strings that were
    // authored with qualified form already.
    let mut pkg_of: BTreeMap<String, Option<String>> = BTreeMap::new();
    for em in &modules {
        pkg_of.insert(em.name.clone(), em.package.clone());
    }

    let mut summaries: Vec<Value> = Vec::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    for em in &modules {
        let qname = qualify_for_graph(em.package.as_deref(), &em.name);
        let layer = em
            .value
            .get("layer")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut sum = Map::new();
        sum.insert("name".into(), Value::String(qname.clone()));
        if let Some(l) = layer {
            sum.insert("layer".into(), Value::String(l));
        }
        if let Some(pkg) = &em.package {
            sum.insert("package".into(), Value::String(pkg.clone()));
        }
        summaries.push(Value::Object(sum));
        if let Some(deps) = em.value.get("depends_on").and_then(Value::as_array) {
            for d in deps {
                let Some(d) = d.as_str() else { continue };
                let (dep_pkg, dep_mod) = packages::parse_ref(d);
                let dep_qname = if dep_pkg.is_some() {
                    // Already qualified (e.g., user-authored `core::Foo`).
                    d.to_string()
                } else if project_has_packages {
                    // Resolve: same package first, else leave bare.
                    let same = em.package.as_deref().filter(|p| {
                        pkg_of
                            .get(dep_mod)
                            .is_some_and(|q| q.as_deref() == Some(*p))
                    });
                    match same {
                        Some(p) => packages::qualify(Some(p), dep_mod),
                        None => match pkg_of.get(dep_mod).and_then(|o| o.clone()) {
                            Some(p) => packages::qualify(Some(&p), dep_mod),
                            None => dep_mod.to_string(),
                        },
                    }
                } else {
                    dep_mod.to_string()
                };
                edges.push((qname.clone(), dep_qname));
            }
        }
    }
    edges.sort();
    edges.dedup();

    // Start with synthesized fields, then merge any additional top-level
    // keys from the raw architecture.toml (domain-specific fields like
    // firmware's `connections` or egui's `mounts` / `navigation`).
    let mut arch_map = Map::new();
    arch_map.insert("modules".into(), Value::Array(summaries));
    arch_map.insert(
        "dependency_graph".into(),
        Value::Array(
            edges
                .into_iter()
                .map(|(a, b)| {
                    let mut e = Map::new();
                    e.insert("from".into(), Value::String(a));
                    e.insert("to".into(), Value::String(b));
                    Value::Object(e)
                })
                .collect(),
        ),
    );

    let arch_file = specs_dir.join("architecture.toml");
    if arch_file.is_file() {
        let raw = canonicalize(load_toml(&arch_file)?);
        if let Value::Object(raw_map) = raw {
            for (k, v) in raw_map {
                // Synthesized keys take precedence; everything else
                // passes through verbatim.
                arch_map.entry(k).or_insert(v);
            }
        }
    }
    let architecture = Value::Object(arch_map);

    // Requirements (flattened) and their external sources.
    let mut reqs: Vec<Value> = Vec::new();
    let mut srcs: Vec<Value> = Vec::new();
    let req_dir = specs_dir.join("requirements");
    if req_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&req_dir)?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let v = canonicalize(load_toml(&path)?);
            if let Some(arr) = v.get("requirement").and_then(Value::as_array) {
                reqs.extend(arr.iter().cloned());
            }
            if let Some(arr) = v.get("source").and_then(Value::as_array) {
                srcs.extend(arr.iter().cloned());
            }
        }
    }
    reqs.sort_by(|a, b| {
        let ka = a.get("id").and_then(Value::as_str).unwrap_or("");
        let kb = b.get("id").and_then(Value::as_str).unwrap_or("");
        ka.cmp(kb)
    });
    srcs.sort_by(|a, b| {
        let ka = a.get("id").and_then(Value::as_str).unwrap_or("");
        let kb = b.get("id").and_then(Value::as_str).unwrap_or("");
        ka.cmp(kb)
    });
    let mut req_map = Map::new();
    req_map.insert("sources".into(), Value::Array(srcs));
    req_map.insert("requirements".into(), Value::Array(reqs));
    let requirements = Value::Object(req_map);

    // Domain-declared extra collections.
    let dom = DomainConfig::load(&inputs.domain_file)?;
    let mut collections: BTreeMap<String, Vec<(String, Value)>> = BTreeMap::new();
    for coll in &dom.collections {
        let mut items: Vec<(String, Value)> = Vec::new();
        let dir = specs_dir.join(&coll.dir);
        if dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&dir)?.filter_map(Result::ok).collect();
            entries.sort_by_key(|e| e.path());
            for entry in entries {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                let value = canonicalize(load_toml(&path)?);
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or("?"))
                    .to_string();
                items.push((name, value));
            }
        }
        items.sort_by(|a, b| a.0.cmp(&b.0));
        collections.insert(coll.name.clone(), items);
    }

    Ok(ExtractionResult {
        modules,
        architecture,
        requirements,
        collections,
    })
}

/// Write all extraction artifacts to `output_dir` as YAML. Returns the paths
/// written, sorted.
pub fn write(result: &ExtractionResult, output_dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating {}", output_dir.display()))?;
    let mut written = Vec::new();

    for em in &result.modules {
        let path = match &em.package {
            Some(pkg) => {
                let pkg_dir = output_dir.join(pkg);
                std::fs::create_dir_all(&pkg_dir)
                    .with_context(|| format!("creating {}", pkg_dir.display()))?;
                pkg_dir.join(format!("{}.yaml", em.name))
            }
            None => output_dir.join(format!("{}.yaml", em.name)),
        };
        let text = to_yaml(&em.value)?;
        std::fs::write(&path, text)?;
        written.push(path);
    }

    let arch_path = output_dir.join("architecture.yaml");
    std::fs::write(&arch_path, to_yaml(&result.architecture)?)?;
    written.push(arch_path);

    let req_path = output_dir.join("requirements.yaml");
    std::fs::write(&req_path, to_yaml(&result.requirements)?)?;
    written.push(req_path);

    for (coll_name, items) in &result.collections {
        let coll_dir = output_dir.join(coll_name);
        std::fs::create_dir_all(&coll_dir)
            .with_context(|| format!("creating {}", coll_dir.display()))?;
        for (item_name, value) in items {
            let path = coll_dir.join(format!("{item_name}.yaml"));
            std::fs::write(&path, to_yaml(value)?)?;
            written.push(path);
        }
    }

    written.sort();
    Ok(written)
}

fn to_yaml(value: &Value) -> Result<String> {
    serde_yaml::to_string(value).map_err(Into::into)
}

fn load_toml(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing TOML {}", path.display()))?;
    let value = serde_json::to_value(&raw)
        .with_context(|| format!("converting TOML to JSON for {}", path.display()))?;
    Ok(value)
}

/// Recursively re-key every object with a `BTreeMap`-sorted order. Arrays
/// keep their order; scalars pass through.
fn canonicalize(value: Value) -> Value {
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
