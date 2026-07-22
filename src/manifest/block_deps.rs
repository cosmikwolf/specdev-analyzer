//! `codegen/<lang>/deps.yaml` — per (block, language) Cargo dep
//! contributions. Schema: `src/embedded/block_deps.schema.yaml`.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

const SCHEMA_YAML: &str = include_str!("../embedded/block_deps.schema.yaml");
const SCHEMA_LABEL: &str = "block_deps.schema.yaml";

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, Default)]
pub struct BlockDeps {
    #[serde(default)]
    pub crates: Vec<CrateEntry>,
    /// pub package contributions to pubspec.yaml (`pub:` key — renamed
    /// because `pub` is a Rust keyword). Runtime `dependencies:` only;
    /// dev-dep routing is a spec-side external_deps concern, mirroring
    /// the crates[] rule.
    #[serde(rename = "pub", default)]
    pub pub_packages: Vec<PubEntry>,
    #[serde(default)]
    pub recommended_external_deps: Vec<RecommendedDep>,
}

/// One pubspec dep entry. The closed set of source shapes matches the
/// embedded schema; the emitter dispatches on the variant — no silent
/// drops.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PubEntry {
    pub package: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(flatten, default)]
    pub source: Option<PubSource>,
}

/// pubspec dep source. Untagged over structurally disjoint required
/// keys (`hosted` / `git` / `path` / `sdk`), so declaration order is
/// not load-bearing.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PubSource {
    Hosted {
        /// Custom package-repository URL (modern short form).
        hosted: String,
    },
    Git {
        git: PubGit,
    },
    Path {
        path: String,
    },
    Sdk {
        sdk: String,
    },
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct PubGit {
    pub url: String,
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub tag_pattern: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CrateEntry {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub default_features: Option<bool>,
    #[serde(default)]
    pub optional: Option<bool>,
    #[serde(default)]
    pub source: Option<CrateSource>,
}

/// Mirrors the existing CrateReference shape in
/// `src/embedded/crate_reference.schema.yaml`. Closed-set tagged union.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum CrateSource {
    Tagged(CrateSourceTag),
    Path {
        path: String,
    },
    Git {
        git: String,
        #[serde(default)]
        rev: Option<String>,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        branch: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum CrateSourceTag {
    #[serde(rename = "workspace")]
    Workspace,
    #[serde(rename = "crates-io")]
    CratesIo,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RecommendedDep {
    /// Crate name; matches a crates.io / git crate name (not validated
    /// against any registry at load time).
    #[serde(rename = "crate")]
    pub crate_name: String,
    /// Required when an entry is listed. Short prose explaining why
    /// /spec-design should suggest this crate alongside the block.
    pub purpose: String,
}

impl BlockDeps {
    /// Load and validate a block `deps.yaml`. Returns
    /// `BlockDeps::default()` if the file is absent — the manifest is
    /// optional (block has no Rust deps to contribute).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let value = super::load_yaml_validated(path, SCHEMA_YAML, SCHEMA_LABEL)?;
        let parsed: BlockDeps = serde_json::from_value(value)
            .with_context(|| format!("deserializing block deps.yaml at {}", path.display()))?;
        Ok(parsed)
    }
}
