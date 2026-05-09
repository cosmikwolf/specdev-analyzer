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
    /// Phase-5 layer_dependency (graph-shape rewrite): entity-side kind
    /// scope. Filters `entities[]` to entries whose kind extends this
    /// kind. Used jointly with `relation_scope` and `layer_field`.
    #[serde(default)]
    pub entity_scope: Option<String>,
    /// Phase-5 layer_dependency: relation-side kind scope. Filters
    /// `relations[]` to entries whose kind extends this kind.
    #[serde(default)]
    pub relation_scope: Option<String>,
    /// Phase-5 layer_dependency: path to the layer string on each
    /// scoped entity (e.g. `attrs.layer`). Looked up against each entity
    /// in the filtered view.
    #[serde(default)]
    pub layer_field: Option<String>,
    /// Phase-5 layer_dependency: layers in declared order, top-most
    /// first (e.g. `["application", "middleware", "driver", "HAL", "PAC"]`).
    /// A relation's `from` layer must appear in `ordering` at a position
    /// less-than-or-equal to its `to` layer's position. Same-layer
    /// dependencies are allowed.
    #[serde(default)]
    pub layer_ordering: Option<Vec<String>>,
    /// Phase 19 — `entity_attr_inbound_count`: filter `entities[]` to
    /// rows whose `entity_attr` equals `entity_attr_value`, then count
    /// inbound `relation_scope` edges per row; assert the count equals
    /// `expected_count`. Authored alongside `decision_supersedes_chain_consistency`.
    #[serde(default)]
    pub entity_attr: Option<String>,
    #[serde(default)]
    pub entity_attr_value: Option<String>,
    #[serde(default)]
    pub expected_count: Option<usize>,
    /// Phase 19 — `entity_attr_target_attr_equals`: filter source entities
    /// by `entity_scope` + `entity_attr == entity_attr_value`; the entity
    /// referenced by `target_attr_field` (an attr on the source entity)
    /// must be of kind `target_entity_kind` and have `target_attr ==
    /// target_attr_value`. Authored alongside `open_question_status_consistency`.
    #[serde(default)]
    pub target_attr_field: Option<String>,
    #[serde(default)]
    pub target_entity_kind: Option<String>,
    #[serde(default)]
    pub target_attr: Option<String>,
    #[serde(default)]
    pub target_attr_value: Option<String>,
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
