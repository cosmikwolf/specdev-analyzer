//! `spec.toml` loader — project-level config.
//!
//! Full override resolution (explicit paths → `.specdev/` → plugin defaults)
//! lands in Phase 7. For now we read whatever paths `spec.toml` provides
//! directly or fall back to caller-supplied `--schema` / `--domain` flags.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct SpecConfig {
    pub spec_version: String,
    pub project: ProjectSection,
    #[serde(default)]
    pub source: SourceSection,
    #[serde(default)]
    pub paths: PathsSection,
    #[serde(default, rename = "packages")]
    pub packages: Vec<PackageSection>,
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

/// A logical package — a filesystem grouping of modules under
/// `specs/modules/<name>/`. Multi-package projects (Rust workspaces,
/// CMake multi-subproject trees) declare one entry per package; single-package
/// projects leave the `[[packages]]` array empty.
#[derive(Debug, Deserialize)]
pub struct PackageSection {
    pub name: String,
    pub source_path: String,
}

impl SpecConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let cfg: SpecConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate_packages()
            .with_context(|| format!("validating {}", path.display()))?;
        Ok(cfg)
    }

    /// Structural validation of `[[packages]]`. Checks name syntax, uniqueness,
    /// and non-empty `source_path`. Does *not* check that `source_path` exists
    /// on disk — that's a validate-time warning (PKG042).
    fn validate_packages(&self) -> Result<()> {
        let mut seen: HashSet<&str> = HashSet::new();
        for pkg in &self.packages {
            if pkg.name.is_empty() {
                bail!("[[packages]] entry has empty `name`");
            }
            if !is_snake_case(&pkg.name) {
                bail!(
                    "[[packages]] name `{}` must be snake_case (lowercase letters, digits, underscores; must start with a letter)",
                    pkg.name
                );
            }
            if !seen.insert(pkg.name.as_str()) {
                bail!("[[packages]] name `{}` declared more than once", pkg.name);
            }
            if pkg.source_path.trim().is_empty() {
                bail!("[[packages]] `{}` has empty `source_path`", pkg.name);
            }
        }
        Ok(())
    }

    /// Look up a declared package by name.
    pub fn find_package(&self, name: &str) -> Option<&PackageSection> {
        self.packages.iter().find(|p| p.name == name)
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

fn is_snake_case(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml_str: &str) -> Result<SpecConfig> {
        let cfg: SpecConfig = toml::from_str(toml_str)?;
        cfg.validate_packages()?;
        Ok(cfg)
    }

    const BASE: &str = r#"
spec_version = "1.0"
[project]
name = "demo"
domain = "firmware"
language = "rust"
"#;

    #[test]
    fn parses_without_packages() {
        let cfg = parse(BASE).unwrap();
        assert!(cfg.packages.is_empty());
        assert!(cfg.find_package("anything").is_none());
    }

    #[test]
    fn parses_multi_package() {
        let src = format!(
            "{BASE}\n[[packages]]\nname = \"core\"\nsource_path = \"crates/core\"\n\
             [[packages]]\nname = \"drivers\"\nsource_path = \"crates/drivers\"\n"
        );
        let cfg = parse(&src).unwrap();
        assert_eq!(cfg.packages.len(), 2);
        assert_eq!(cfg.find_package("core").unwrap().source_path, "crates/core");
        assert_eq!(
            cfg.find_package("drivers").unwrap().source_path,
            "crates/drivers"
        );
    }

    #[test]
    fn rejects_duplicate_package_name() {
        let src = format!(
            "{BASE}\n[[packages]]\nname = \"core\"\nsource_path = \"crates/core\"\n\
             [[packages]]\nname = \"core\"\nsource_path = \"crates/core2\"\n"
        );
        let err = parse(&src).unwrap_err().to_string();
        assert!(err.contains("declared more than once"), "got: {err}");
    }

    #[test]
    fn rejects_empty_source_path() {
        let src = format!("{BASE}\n[[packages]]\nname = \"core\"\nsource_path = \"\"\n");
        let err = parse(&src).unwrap_err().to_string();
        assert!(err.contains("empty `source_path`"), "got: {err}");
    }

    #[test]
    fn rejects_non_snake_case_name() {
        let src = format!("{BASE}\n[[packages]]\nname = \"Core\"\nsource_path = \"crates/core\"\n");
        let err = parse(&src).unwrap_err().to_string();
        assert!(err.contains("snake_case"), "got: {err}");
    }

    #[test]
    fn accepts_digit_and_underscore_in_name() {
        let src = format!("{BASE}\n[[packages]]\nname = \"pkg_v2\"\nsource_path = \"crates/v2\"\n");
        let cfg = parse(&src).unwrap();
        assert_eq!(cfg.packages.len(), 1);
    }

    #[test]
    fn is_snake_case_rejects_leading_digit_or_underscore() {
        assert!(!is_snake_case("1pkg"));
        assert!(!is_snake_case("_pkg"));
        assert!(!is_snake_case(""));
        assert!(is_snake_case("pkg"));
        assert!(is_snake_case("a_b_c"));
    }
}
