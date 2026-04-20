//! `spec.toml` loader — project-level config.
//!
//! Full override resolution (explicit paths → `.specdev/` → plugin defaults)
//! lands in Phase 7. For now we read whatever paths `spec.toml` provides
//! directly or fall back to caller-supplied `--schema` / `--domain` flags.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SpecConfig {
    pub spec_version: String,
    pub project: ProjectSection,
    #[serde(default)]
    pub source: SourceSection,
    #[serde(default)]
    pub paths: PathsSection,
}

#[derive(Debug, Deserialize)]
pub struct ProjectSection {
    pub name: String,
    pub domain: String,
    pub language: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct SourceSection {
    #[serde(default)]
    pub root: Option<String>,
    #[serde(default)]
    pub layout: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PathsSection {
    #[serde(default)]
    pub overrides: OverrideSection,
}

#[derive(Debug, Default, Deserialize)]
pub struct OverrideSection {
    #[serde(default)]
    pub schema: Option<String>,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub codegen: Option<String>,
}

impl SpecConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: SpecConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    /// Major version prefix used for the versioned schema directory.
    /// "1.0" → "v1".
    pub fn schema_major(&self) -> String {
        let major = self
            .spec_version
            .split('.')
            .next()
            .unwrap_or(&self.spec_version);
        format!("v{major}")
    }
}

/// Locate `spec.toml` by walking up from `start`.
pub fn find_spec_toml(start: &Path) -> Option<PathBuf> {
    let start = std::fs::canonicalize(start).ok()?;
    let mut dir = start.as_path();
    loop {
        let candidate = dir.join("spec.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return None,
        }
    }
}
