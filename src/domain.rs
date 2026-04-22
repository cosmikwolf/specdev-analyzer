//! `domain.toml` loader.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DomainConfig {
    pub domain: DomainInfo,
    #[serde(default)]
    pub schemas: BTreeMap<String, String>,
    #[serde(default)]
    pub layers: LayersSection,
    #[serde(default)]
    pub required_metadata: RequiredMetadata,
    #[serde(default)]
    pub validation: ValidationSection,
    #[serde(default)]
    pub module: ModuleSection,
    #[serde(default)]
    pub project: ProjectSection,
    /// Additional per-file spec collections beyond the built-in
    /// `specs/modules/`, `specs/requirements/`, and `specs/architecture.toml`
    /// that the analyzer has always scanned. Each collection declares a
    /// subdirectory under `specs/` and a schema key to validate entries
    /// against. Entries appear at `project.<name>[]` for project-level
    /// graph checks, and are extracted to `<out>/<name>/<Item>.yaml`.
    #[serde(default, rename = "collections")]
    pub collections: Vec<Collection>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Collection {
    /// Key the collection occupies in the project composite (e.g. `stores`).
    pub name: String,
    /// Subdirectory under `specs/` to scan (e.g. `stores`).
    pub dir: String,
    /// Schema registry key each file is validated against (e.g. `store`).
    pub schema: String,
}

#[derive(Debug, Deserialize)]
pub struct DomainInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct LayersSection {
    #[serde(default)]
    pub order: Vec<String>,
    #[serde(default)]
    pub allowed_deps: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct RequiredMetadata {
    #[serde(default)]
    pub module: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ValidationSection {
    #[serde(default)]
    pub rules: BTreeMap<String, Severity>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Default, Deserialize)]
pub struct ModuleSection {
    #[serde(default, rename = "graph_checks")]
    pub graph_checks: Vec<GraphCheck>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProjectSection {
    #[serde(default, rename = "graph_checks")]
    pub graph_checks: Vec<GraphCheck>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GraphCheck {
    pub kind: String,
    #[serde(default)]
    pub rule: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    // Field selectors used by different primitives:
    #[serde(default)]
    pub field: Option<String>,
    #[serde(default)]
    pub source_field: Option<String>,
    #[serde(default)]
    pub target_field: Option<String>,
    #[serde(default)]
    pub nodes_field: Option<String>,
    #[serde(default)]
    pub edges_from: Option<String>,
    #[serde(default)]
    pub edges_to: Option<String>,
    #[serde(default)]
    pub entry_field: Option<String>,
    #[serde(default)]
    pub module_layer_field: Option<String>,
    #[serde(default)]
    pub module_deps_field: Option<String>,
    #[serde(default)]
    pub allowed_deps_table: Option<String>,
}

impl DomainConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: DomainConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    pub fn severity_of(&self, rule: &str) -> Severity {
        self.validation
            .rules
            .get(rule)
            .copied()
            .unwrap_or(Severity::Error)
    }
}
