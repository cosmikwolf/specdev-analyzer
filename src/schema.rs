//! JSON Schema loader + validator.
//!
//! Schemas live as YAML next to each other and cross-reference via filename
//! refs like `enums.schema.yaml#/$defs/Identifier`. We load every
//! `*.schema.yaml` in the configured directory, register each one by both its
//! declared `$id` and its filename-based URI, then compile whichever schema
//! the caller asks to validate against.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use jsonschema::{Retrieve, Uri, Validator};
use serde_json::Value;

use crate::domain::Severity;
use crate::report::Finding;

/// Resolves `$ref` targets by URI or bare filename against a pre-loaded
/// document map. We register every schema under both its declared `$id`
/// and its filename so cross-file refs like `enums.schema.yaml#/$defs/X`
/// resolve without any network I/O.
struct MapRetriever {
    docs: Arc<HashMap<String, Value>>,
}

impl Retrieve for MapRetriever {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let s = uri.as_str();
        if let Some(v) = self.docs.get(s) {
            return Ok(v.clone());
        }
        // Fall back to the last path segment (e.g. `file:///x/enums.schema.yaml`
        // → `enums.schema.yaml`) so references that got resolved against a
        // synthetic base URI still land on our filename-keyed entries.
        if let Some(tail) = s.rsplit('/').next() {
            if let Some(v) = self.docs.get(tail) {
                return Ok(v.clone());
            }
        }
        Err(format!("schema not found: {s}").into())
    }
}

pub struct SchemaRegistry {
    /// Every loaded schema keyed by its `$id` and its filename (both point at
    /// the same value). The Retrieve impl resolves against this map.
    docs: Arc<HashMap<String, Value>>,
    /// "module" → the parsed schema value. Used to pick which schema to compile.
    by_name: HashMap<String, Value>,
    /// Lazily-built validators keyed by schema name. Compiling a jsonschema
    /// is expensive (tens of ms on our schemas), so we cache after the first
    /// call and reuse across all files of the same kind.
    compiled: RefCell<HashMap<String, Arc<Validator>>>,
}

impl SchemaRegistry {
    /// Load every `*.schema.yaml` from `dir`.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut docs: HashMap<String, Value> = HashMap::new();
        let mut by_name = HashMap::new();

        let entries = std::fs::read_dir(dir)
            .with_context(|| format!("reading schema dir {}", dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !is_schema_yaml(&path) {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let value: Value = serde_yaml::from_str(&text)
                .with_context(|| format!("parsing {}", path.display()))?;

            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            by_name.insert(schema_key(&fname), value.clone());

            if let Some(id) = value.get("$id").and_then(Value::as_str) {
                docs.insert(id.to_string(), value.clone());
            }
            docs.insert(fname, value);
        }

        Ok(Self {
            docs: Arc::new(docs),
            by_name,
            compiled: RefCell::new(HashMap::new()),
        })
    }

    fn raw(&self, name: &str) -> Option<&Value> {
        self.by_name.get(name)
    }

    fn compile(&self, name: &str, schema: &Value) -> Result<Validator> {
        let retriever = MapRetriever {
            docs: Arc::clone(&self.docs),
        };
        jsonschema::options()
            .with_retriever(retriever)
            .build(schema)
            .map_err(|e| anyhow::anyhow!("compiling schema `{name}`: {e}"))
    }

    fn validator(&self, name: &str) -> Result<Arc<Validator>> {
        if let Some(v) = self.compiled.borrow().get(name) {
            return Ok(Arc::clone(v));
        }
        let Some(raw) = self.raw(name) else {
            let known: Vec<&str> = self.by_name.keys().map(String::as_str).collect();
            return Err(anyhow::anyhow!(
                "schema `{name}` not loaded — check `[schemas]` in domain.toml. Loaded schemas: {known:?}"
            ));
        };
        let v = Arc::new(self.compile(name, raw)?);
        self.compiled
            .borrow_mut()
            .insert(name.to_string(), Arc::clone(&v));
        Ok(v)
    }

    /// Validate `value` against the schema named `name` (e.g. "module").
    /// Emits one Finding per structural error.
    pub fn validate(&self, name: &str, value: &Value, file: &str) -> Result<Vec<Finding>> {
        let compiled = self.validator(name)?;
        let mut out = Vec::new();
        for err in compiled.iter_errors(value) {
            out.push(
                Finding::new(
                    "SCHEMA",
                    Severity::Error,
                    crate::diagnostics::format_schema_error(&err),
                )
                .with_file(file.to_string())
                .with_pointer(err.instance_path().to_string()),
            );
        }
        Ok(out)
    }
}

fn is_schema_yaml(path: &Path) -> bool {
    let s = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    s.ends_with(".schema.yaml") || s.ends_with(".schema.yml")
}

/// Turn `module.schema.yaml` → `module`.
fn schema_key(filename: &str) -> String {
    filename
        .trim_end_matches(".yaml")
        .trim_end_matches(".yml")
        .trim_end_matches(".schema")
        .to_string()
}

/// Convenience: resolve schema paths from `domain.schemas` map into a
/// directory. Panics if the map is empty or entries diverge.
pub fn schemas_dir(
    domain_file: &Path,
    schemas: &std::collections::BTreeMap<String, String>,
) -> Option<PathBuf> {
    let first = schemas.values().next()?;
    let base = domain_file.parent()?;
    let joined = base.join(first);
    joined.parent().map(Path::to_path_buf)
}
