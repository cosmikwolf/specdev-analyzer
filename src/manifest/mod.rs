//! Manifest loaders.
//!
//! Three runtime-loaded YAML manifests drive the analyzer's
//! profile/block/bridge dispatch. This module ships the
//! deserialization + validation surface; dispatch wiring lives in the
//! consumer modules (`cargo_toml.rs`, `pubspec_yaml.rs`,
//! `package_json.rs`, `skeleton.rs`, `context.rs`, `bootstrap.rs`,
//! `reduction.rs`, `validate.rs`).
//!
//! [`has_workspace_ancestor`] is the one piece of dispatch logic that
//! lives here: the pub and npm emitters both need "is this directory a
//! member of an enclosing spec.toml `[workspace]`?" and nothing else
//! about the workspace.
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
//! Tests assert round-trip against the draft manifests.

pub mod block_deps;
pub mod bridge_contract;
pub mod profile;

pub use profile::{CrateType as ProfileCrateType, LibSection};
pub use block_deps::{
    BlockDeps, CrateEntry, CrateSource, CrateSourceTag, NpmEntry, NpmGit, NpmSource, PubEntry,
    PubGit, PubSource, RecommendedDep,
};
pub use bridge_contract::{BindingLocality, BridgeContract};
pub use profile::{
    BinGate, BinSection, BootstrapSection, BootstrapStep, CargoTomlSection, ContextExtra,
    ContextSection, DerivedRule, FeaturesSection, Gate, GateScope, ItemsSection, NpmEngines, NpmModuleType,
    PackageJsonSection, PackageJsonShapeKind, PairedFile, PairedStub, Profile, PubEnvironment,
    PubShapeKind, PubspecYamlSection, ReverseSection, ReverseVocabulary, ShapeKind, SurfaceSection,
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
    let value = read_yaml(path)?;
    validate_against(&value, path, schema_yaml, schema_label)?;
    Ok(value)
}

/// Read + YAML-parse a manifest, with no schema validation. Split out of
/// [`load_yaml_validated`] so a caller can inspect the parsed value
/// *before* the validator runs — `profile.yaml`'s `contract_version`
/// handshake has to, because a version mismatch would otherwise present
/// as an unreadable pile of `additionalProperties` errors (#117).
pub(crate) fn read_yaml(path: &Path) -> Result<JsonValue> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    serde_yaml::from_str(&text)
        .with_context(|| format!("parsing {} as YAML", path.display()))
}

/// Validate an already-parsed manifest against an embedded schema.
/// `MANIFEST-001` on failure, capped at five errors.
pub(crate) fn validate_against(
    value: &JsonValue,
    path: &Path,
    schema_yaml: &'static str,
    schema_label: &'static str,
) -> Result<()> {
    let validator = compile_schema(schema_yaml, schema_label);
    let errors: Vec<_> = validator.iter_errors(value).collect();
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
    Ok(())
}

/// True when an ancestor spec.toml carries a `[workspace]` whose
/// `members[].path` includes `member_dir`. Deliberately NOT the cargo
/// path's `WorkspaceContext` — that walker also parses the workspace's
/// `[[workspace_dependencies]]` as *cargo* refs, which would fail with
/// cargo-vocabulary errors on a Dart or TypeScript workspace. Member
/// mode needs only the membership boolean.
pub fn has_workspace_ancestor(member_dir: &Path) -> Result<bool> {
    let mut cursor = match member_dir.parent() {
        Some(p) => p,
        None => return Ok(false),
    };
    loop {
        let candidate = cursor.join("spec.toml");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            if let Ok(parsed) = toml::from_str::<toml::Value>(&text) {
                let members = parsed
                    .get("workspace")
                    .and_then(|w| w.get("members"))
                    .and_then(|m| m.as_array());
                if let Some(members) = members {
                    let root = candidate.parent().unwrap_or(&candidate);
                    if let Ok(rel) = member_dir.strip_prefix(root) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if members.iter().any(|m| {
                            m.get("path")
                                .and_then(|p| p.as_str())
                                .map(|p| p.trim_end_matches('/') == rel)
                                .unwrap_or(false)
                        }) {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        match cursor.parent() {
            Some(p) => cursor = p,
            None => return Ok(false),
        }
    }
}
