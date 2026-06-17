//! Embedded, project-independent JSON Schemas registered in retriever
//! maps so `$ref`s to their `$id` resolve regardless of project content.
//!
//! The first such schema is `CrateReference`, a
//! reusable struct shared across profile-level architecture schemas
//! and module-level `external_deps`. The aggregator and the
//! architecture-validation pipelines both register the entries returned
//! by [`embedded_schema_entries`] in their `$ref` retrievers.
//!
//! Adding another embedded schema means extending `EMBEDDED` below;
//! both retrievers pick it up automatically.

use std::sync::OnceLock;

use serde_json::Value as JsonValue;

const CRATE_REFERENCE_YAML: &str = include_str!("embedded/crate_reference.schema.yaml");

/// Static list of `(filename, raw YAML source)` pairs for every embedded
/// schema that participates in `$ref` resolution.
const EMBEDDED: &[(&str, &str)] = &[("crate_reference.schema.yaml", CRATE_REFERENCE_YAML)];

/// Parsed entries: `(string-keys, JSON Value)` pairs ready to insert into
/// a retriever doc map. Each schema is registered under both its `$id`
/// (canonical) and its filename (fallback).
pub fn embedded_schema_entries() -> &'static [(Vec<String>, JsonValue)] {
    static CELL: OnceLock<Vec<(Vec<String>, JsonValue)>> = OnceLock::new();
    CELL.get_or_init(|| {
        EMBEDDED
            .iter()
            .map(|(fname, yaml)| {
                let value: JsonValue = serde_yaml::from_str(yaml)
                    .unwrap_or_else(|e| panic!("embedded {fname} failed to parse: {e}"));
                let mut keys = vec![(*fname).to_string()];
                if let Some(id) = value.get("$id").and_then(JsonValue::as_str) {
                    keys.push(id.to_string());
                }
                (keys, value)
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_reference_is_registered_under_its_id() {
        let entries = embedded_schema_entries();
        let cr = entries
            .iter()
            .find(|(keys, _)| keys.iter().any(|k| k == "crate_reference.schema.yaml"))
            .expect("CrateReference entry present");
        assert!(
            cr.0.iter()
                .any(|k| k == "https://specdev.dev/schema/meta/v1/crate_reference.schema.yaml"),
            "CrateReference must be registered under its $id; got keys: {:?}",
            cr.0
        );
        assert!(cr.1.pointer("/$defs/CrateReference").is_some());
    }
}
