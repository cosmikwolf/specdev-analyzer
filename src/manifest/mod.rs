//! Phase 16 manifest loaders (D52 / doc 21).
//!
//! Three runtime-loaded YAML manifests drive the analyzer's
//! profile/block/bridge dispatch in Phase 16. This module ships the
//! deserialization + validation surface; dispatch wiring lives in the
//! consumer modules (`cargo_toml.rs`, `skeleton.rs`, `context.rs`,
//! `bootstrap.rs`, `reduction.rs`, `validate.rs`) and lands in
//! subsequent §8.1 steps.
//!
//! Each manifest:
//!   - has its YAML JSON Schema under `src/embedded/`
//!   - has a strongly-typed Rust struct here
//!   - loads via `<Manifest>::load(&path) -> Result<Self>` which:
//!       1. reads and YAML-parses the file,
//!       2. validates against the embedded schema (load-time),
//!       3. deserializes into the struct.
//!
//! Errors are anyhow::Error with surface that names the file path + line.
//! Schema violations come with closed-enum hints from jsonschema's output.
//!
//! Step 1 (this module) is purely additive: nothing in the existing
//! analyzer pipeline calls these loaders yet. Tests assert round-trip
//! against the 10 draft manifests in doc 21 §3.3 / §4.2 / §5.3.

pub mod block_deps;
pub mod bridge_contract;
pub mod profile;

pub use block_deps::{BlockDeps, CrateEntry, CrateSource, CrateSourceTag, RecommendedDep};
pub use bridge_contract::{BindingLocality, BridgeContract};
pub use profile::{
    BinGate, BinSection, BootstrapSection, BootstrapStep, CargoTomlSection,
    ContextExtra, ContextSection, FeaturesSection, PairedStub,
    Profile, ShapeKind, SurfaceSection,
};

use anyhow::{anyhow, Context, Result};
use jsonschema::Validator;
use serde_json::Value as JsonValue;
use std::path::Path;
use std::sync::OnceLock;

/// Compile a JSON Schema from raw YAML bytes once and cache the
/// validator. Used by each manifest loader to validate parsed YAML
/// before serde-deserializing into the typed struct.
fn compile_schema(yaml: &str, label: &'static str) -> &'static Validator {
    static CACHE: OnceLock<std::sync::Mutex<std::collections::HashMap<&'static str, &'static Validator>>> =
        OnceLock::new();
    let map = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let mut guard = map.lock().expect("manifest schema cache poisoned");
    if let Some(v) = guard.get(label) {
        return v;
    }
    let value: JsonValue = serde_yaml::from_str(yaml)
        .unwrap_or_else(|e| panic!("embedded {label} failed to parse: {e}"));
    let validator = jsonschema::options()
        .build(&value)
        .unwrap_or_else(|e| panic!("embedded {label} failed to compile: {e}"));
    let leaked: &'static Validator = Box::leak(Box::new(validator));
    guard.insert(label, leaked);
    leaked
}

/// Load a manifest file: read bytes, YAML-parse to JsonValue, validate
/// against the embedded schema, then return the JsonValue for the
/// caller to serde-deserialize into a typed struct. Centralizes the
/// shared "read + parse + validate" sequence so the per-manifest
/// loaders stay focused on shape.
fn load_yaml_validated(
    path: &Path,
    schema_yaml: &'static str,
    schema_label: &'static str,
) -> Result<JsonValue> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let value: JsonValue = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing {} as YAML", path.display()))?;
    let validator = compile_schema(schema_yaml, schema_label);
    let errors: Vec<_> = validator.iter_errors(&value).collect();
    if !errors.is_empty() {
        let mut msg = format!(
            "[MANIFEST-001] {} failed schema validation against {}:\n",
            path.display(),
            schema_label,
        );
        for err in errors.iter().take(5) {
            msg.push_str(&format!("  - {} at {}\n", err, err.instance_path()));
        }
        if errors.len() > 5 {
            msg.push_str(&format!("  ... and {} more\n", errors.len() - 5));
        }
        return Err(anyhow!(msg));
    }
    Ok(value)
}
