//! JSON Schema loader + validator.
//!
//! Schemas live as YAML next to each other and cross-reference via filename
//! refs like `enums.schema.yaml#/$defs/Identifier`. We load every
//! `*.schema.yaml` in the configured directory, register each one by both its
//! declared `$id` and its filename-based URI, then compile whichever schema
//! the caller asks to validate against.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use jsonschema::JSONSchema;
use serde_json::Value;

use crate::domain::Severity;
use crate::report::Finding;

pub struct SchemaRegistry {
    /// Every loaded schema keyed by its `$id` (or filename fallback). All of
    /// these are registered as external documents when compiling any one
    /// schema, so cross-file `$ref`s resolve.
    docs: Vec<(String, Value)>,
    /// Path (filename stem + extension) → compiled schema, lazy filled.
    /// We compile on demand because compilation is not trivially cheap.
    by_name: HashMap<String, Value>,
}

impl SchemaRegistry {
    /// Load every `*.schema.yaml` from `dir`.
    pub fn load_dir(dir: &Path) -> Result<Self> {
        let mut docs = Vec::new();
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
                docs.push((id.to_string(), value.clone()));
            }
            docs.push((fname, value));
        }

        Ok(Self { docs, by_name })
    }

    fn raw(&self, name: &str) -> Option<&Value> {
        self.by_name.get(name)
    }

    fn compile(&self, schema: &Value) -> Result<JSONSchema> {
        let mut opts = JSONSchema::options();
        for (uri, doc) in &self.docs {
            opts.with_document(uri.clone(), doc.clone());
        }
        opts.compile(schema)
            .map_err(|e| anyhow::anyhow!("compiling schema: {e}"))
    }

    /// Validate `value` against the schema named `name` (e.g. "module").
    /// Emits one Finding per structural error.
    pub fn validate(
        &self,
        name: &str,
        value: &Value,
        file: &str,
    ) -> Result<Vec<Finding>> {
        let Some(raw) = self.raw(name) else {
            return Err(anyhow::anyhow!("schema `{name}` not loaded"));
        };
        let compiled = self.compile(raw)?;
        let mut out = Vec::new();
        if let Err(errors) = compiled.validate(value) {
            for err in errors {
                out.push(
                    Finding::new(
                        "SCHEMA",
                        Severity::Error,
                        format!("{err}"),
                    )
                    .with_file(file.to_string())
                    .with_pointer(err.instance_path.to_string()),
                );
            }
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
pub fn schemas_dir(domain_file: &Path, schemas: &std::collections::BTreeMap<String, String>) -> Option<PathBuf> {
    let first = schemas.values().next()?;
    let base = domain_file.parent()?;
    let joined = base.join(first);
    joined.parent().map(Path::to_path_buf)
}
