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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::validate::{self, Inputs};

pub struct ExtractionResult {
    /// (module_name, module value) pairs, sorted by name.
    pub modules: Vec<(String, Value)>,
    pub architecture: Value,
    pub requirements: Value,
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

    // Modules.
    let mut modules: Vec<(String, Value)> = Vec::new();
    let modules_dir = specs_dir.join("modules");
    if modules_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&modules_dir)?
            .filter_map(Result::ok)
            .collect();
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
            modules.push((name, value));
        }
    }
    modules.sort_by(|a, b| a.0.cmp(&b.0));

    // Architecture summary (synthesized).
    let mut summaries: Vec<Value> = Vec::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    for (_, m) in &modules {
        let name = m.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let layer = m.get("layer").and_then(Value::as_str).map(str::to_string);
        let mut sum = Map::new();
        sum.insert("name".into(), Value::String(name.clone()));
        if let Some(l) = layer {
            sum.insert("layer".into(), Value::String(l));
        }
        summaries.push(Value::Object(sum));
        if let Some(deps) = m.get("depends_on").and_then(Value::as_array) {
            for d in deps {
                if let Some(d) = d.as_str() {
                    edges.push((name.clone(), d.to_string()));
                }
            }
        }
    }
    edges.sort();
    edges.dedup();

    // Raw connections from specs/architecture.toml (if present).
    let mut connections = Value::Array(Vec::new());
    let arch_file = specs_dir.join("architecture.toml");
    if arch_file.is_file() {
        let raw = canonicalize(load_toml(&arch_file)?);
        if let Some(c) = raw.get("connections") {
            connections = c.clone();
        }
    }

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
    arch_map.insert("connections".into(), connections);
    let architecture = Value::Object(arch_map);

    // Requirements (flattened).
    let mut reqs: Vec<Value> = Vec::new();
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
        }
    }
    reqs.sort_by(|a, b| {
        let ka = a.get("id").and_then(Value::as_str).unwrap_or("");
        let kb = b.get("id").and_then(Value::as_str).unwrap_or("");
        ka.cmp(kb)
    });
    let mut req_map = Map::new();
    req_map.insert("requirements".into(), Value::Array(reqs));
    let requirements = Value::Object(req_map);

    Ok(ExtractionResult {
        modules,
        architecture,
        requirements,
    })
}

/// Write all extraction artifacts to `output_dir` as YAML. Returns the paths
/// written, sorted.
pub fn write(result: &ExtractionResult, output_dir: &Path) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating {}", output_dir.display()))?;
    let mut written = Vec::new();

    for (name, value) in &result.modules {
        let path = output_dir.join(format!("{name}.yaml"));
        let text = to_yaml(value)?;
        std::fs::write(&path, text)?;
        written.push(path);
    }

    let arch_path = output_dir.join("architecture.yaml");
    std::fs::write(&arch_path, to_yaml(&result.architecture)?)?;
    written.push(arch_path);

    let req_path = output_dir.join("requirements.yaml");
    std::fs::write(&req_path, to_yaml(&result.requirements)?)?;
    written.push(req_path);

    written.sort();
    Ok(written)
}

fn to_yaml(value: &Value) -> Result<String> {
    serde_yaml::to_string(value).map_err(Into::into)
}

fn load_toml(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let raw: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parsing TOML {}", path.display()))?;
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
        Value::Array(items) => {
            Value::Array(items.into_iter().map(canonicalize).collect())
        }
        other => other,
    }
}
