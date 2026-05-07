//! `contract.yaml` — bridge dispatch metadata.
//! Schema: `src/embedded/bridge_contract.schema.yaml`.
//! Authoritative spec: doc 21 §5.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

const SCHEMA_YAML: &str = include_str!("../embedded/bridge_contract.schema.yaml");
const SCHEMA_LABEL: &str = "bridge_contract.schema.yaml";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct BridgeContract {
    pub binding_locality: BindingLocality,
    pub diagnostic_template: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BindingLocality {
    EndpointModule,
    PluginShellUnique,
}

impl BridgeContract {
    /// Load and validate a bridge `contract.yaml`. Path is the file
    /// itself (typically `specs/specdev/reductions/bridges/<bridge>.contract.yaml`
    /// in the inlined tree, or `bridges/<a>+<b>/contract.yaml` in the
    /// plugin tree).
    pub fn load(path: &Path) -> Result<Self> {
        let value = super::load_yaml_validated(path, SCHEMA_YAML, SCHEMA_LABEL)?;
        let parsed: BridgeContract = serde_json::from_value(value)
            .with_context(|| format!("deserializing contract.yaml at {}", path.display()))?;
        Ok(parsed)
    }
}
