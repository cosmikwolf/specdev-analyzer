//! `spec.toml` loader — project-level manifest.
//!
//! Phase 1 of the blocks + bridges + profiles arc. `spec.toml` is parsed and
//! validated against an embedded JSON Schema that the analyzer ships
//! (`src/embedded/spec.schema.yaml`). Two-pass validation:
//!
//! 1. `toml::from_str` — catches structural TOML errors with byte-span
//!    pointers, rendered as `<path>:<line>:<col>: <msg>` via [`LocatedError`].
//! 2. JSON Schema validation of the deserialized JSON tree — catches missing
//!    fields, value-shape errors, and unknown keys (`additionalProperties:
//!    false`).
//!
//! There is **no resolution chain at analyze time** (doc 15 §11 invariant 5):
//! `spec.toml` carries the entire project shape directly, with `[paths].specs`
//! and `[paths].source` resolved relative to the manifest's location.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::diagnostics::LocatedError;

const SPEC_SCHEMA_YAML: &str = include_str!("embedded/spec.schema.yaml");

#[derive(Debug, Clone, Deserialize)]
pub struct SpecConfig {
    pub spec_version: String,
    pub project: ProjectSection,
    pub paths: PathsSection,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectSection {
    pub name: String,
    pub profile: String,
    pub language: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PathsSection {
    /// Directory containing the project's `specs/` tree (machinery +
    /// modules), relative to `spec.toml`. Conventionally `"specs"`.
    pub specs: String,
    /// Source-code directory the analyzer audits and reverse-engineers
    /// against, relative to `spec.toml`. Conventionally `"src"`.
    pub source: String,
    /// Optional plugin-root override (path relative to `spec.toml`).
    /// Used by `bootstrap` / `inline` / `inline-bridge` when no
    /// `--plugin-root` flag is passed. See [`crate::plugin_root`] for the
    /// full resolution chain.
    #[serde(default)]
    pub plugin_root: Option<String>,
}

impl SpecConfig {
    /// Load `spec.toml` from a path. Returns a [`LocatedError`]-bearing
    /// `anyhow::Error` on failure so the CLI's `report_error` helper can
    /// render `<path>:<line>:<col>: <msg>`.
    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(path, &text)
    }

    /// Parse + validate from in-memory text. Useful for tests.
    pub fn parse(path: &Path, text: &str) -> Result<Self> {
        // Pass 1: TOML parse → JSON value (for schema validation) and a typed
        // value (returned to caller). Use `toml::de` directly so we can recover
        // byte spans on error.
        let raw_json: serde_json::Value = match toml::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                return Err(anyhow!(LocatedError::from_toml_error(path, text, &e))
                    .context(format!("parsing {}", path.display())));
            }
        };

        // Pass 2: validate against the embedded JSON Schema.
        let validator = spec_schema_validator();
        if let Err(err) = validator.validate(&raw_json) {
            // The jsonschema 0.46 API: `validate` returns the first error; use
            // `iter_errors` for the full set. Phase 1 surfaces the first one
            // with file:line precision; later phases can fan out to multi-error
            // reporting per finding.
            let pointer = err.instance_path().to_string();
            let (line, col) = locate_error_in_toml(text, &pointer);
            let field = humanize_pointer(&pointer);
            let msg = format!("{}: {}", field, err);
            return Err(anyhow!(LocatedError::new(path, line, col, msg))
                .context(format!("validating {}", path.display())));
        }

        // Now that the schema accepted the document, deserialize into the
        // typed shape. This cannot fail for any document the schema accepts.
        let cfg: SpecConfig = serde_json::from_value(raw_json)
            .with_context(|| format!("deserializing {}", path.display()))?;
        Ok(cfg)
    }
}

fn spec_schema_validator() -> &'static jsonschema::Validator {
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    VALIDATOR.get_or_init(|| {
        let schema_yaml: serde_json::Value = serde_yaml::from_str(SPEC_SCHEMA_YAML)
            .expect("embedded spec.schema.yaml parses as YAML");
        jsonschema::options()
            .build(&schema_yaml)
            .expect("embedded spec.schema.yaml is a well-formed JSON Schema")
    })
}

/// Best-effort line/col lookup for a JSON pointer error in TOML source.
/// JSON Schema gives us a pointer like `/project/profile`; we walk the source
/// for the first occurrence of `[project]` to anchor the line. If no anchor
/// is found, return `(1, 1)` so the user still sees the file path.
fn locate_error_in_toml(source: &str, pointer: &str) -> (u32, u32) {
    if pointer.is_empty() {
        return (1, 1);
    }
    let mut parts = pointer.split('/').filter(|s| !s.is_empty());
    let Some(top) = parts.next() else {
        return (1, 1);
    };
    let table_open = format!("[{top}]");
    let table_array_open = format!("[[{top}]]");
    for (idx, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with(&table_open) || trimmed.starts_with(&table_array_open) {
            return ((idx + 1) as u32, 1);
        }
    }
    // Fall back to the top-level `key =` form.
    for (idx, line) in source.lines().enumerate() {
        if line.trim_start().starts_with(&format!("{top} =")) {
            return ((idx + 1) as u32, 1);
        }
    }
    (1, 1)
}

fn humanize_pointer(pointer: &str) -> String {
    if pointer.is_empty() {
        return "spec.toml".to_string();
    }
    let segs: Vec<&str> = pointer.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() <= 1 {
        return segs.join(".");
    }
    // /project/profile → [project].profile
    let head = segs[0];
    let tail = segs[1..].join(".");
    format!("[{head}].{tail}")
}

/// Locate `spec.toml` by walking up from `start`. Returns `None` if no
/// manifest is found before reaching the filesystem root.
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

/// Discover `spec.toml` honoring an explicit `--spec-path` override.
/// `explicit` short-circuits the CWD walk; otherwise [`find_spec_toml`] is
/// used. Returns the canonicalized absolute path on success.
pub fn discover_spec_toml(start: &Path, explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if !p.is_file() {
            anyhow::bail!(
                "--spec-path {} does not exist or is not a file",
                p.display()
            );
        }
        return std::fs::canonicalize(p).with_context(|| format!("canonicalizing {}", p.display()));
    }
    find_spec_toml(start).ok_or_else(|| {
        anyhow!(
            "no spec.toml found at or above {} — pass --spec-path to point at one explicitly",
            start.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
spec_version = "1.0"

[project]
name = "demo"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#;

    fn parse_str(text: &str) -> Result<SpecConfig> {
        SpecConfig::parse(Path::new("/tmp/spec.toml"), text)
    }

    #[test]
    fn embedded_schema_loads() {
        let _v = spec_schema_validator();
    }

    #[test]
    fn parses_minimal_well_formed_spec() {
        let cfg = parse_str(GOOD).expect("good spec parses");
        assert_eq!(cfg.spec_version, "1.0");
        assert_eq!(cfg.project.name, "demo");
        assert_eq!(cfg.project.profile, "firmware-layered");
        assert_eq!(cfg.project.language, "rust");
        assert_eq!(cfg.paths.specs, "specs");
        assert_eq!(cfg.paths.source, "src");
    }

    #[test]
    fn rejects_missing_profile_with_field_name_in_message() {
        let bad = r#"
spec_version = "1.0"

[project]
name = "demo"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#;
        let err = parse_str(bad).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("project"), "missing-field message: {s}");
    }

    #[test]
    fn rejects_legacy_domain_field_with_unknown_key() {
        // The new schema is closed (`additionalProperties: false`); any
        // prior-arc `[project].domain` field must reject.
        let bad = r#"
spec_version = "1.0"

[project]
name = "demo"
profile = "firmware-layered"
language = "rust"
domain = "firmware"

[paths]
specs = "specs"
source = "src"
"#;
        let err = parse_str(bad).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("project"), "{s}");
    }

    #[test]
    fn rejects_unknown_language() {
        let bad = r#"
spec_version = "1.0"

[project]
name = "demo"
profile = "firmware-layered"
language = "ada"

[paths]
specs = "specs"
source = "src"
"#;
        let err = parse_str(bad).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("project") || s.contains("language"), "{s}");
    }

    #[test]
    fn rejects_bad_syntax_with_file_line_col() {
        let bad = r#"
spec_version = "1.0"

[project
name = "demo"
"#;
        let err = parse_str(bad).unwrap_err();
        let loc = err.downcast_ref::<LocatedError>();
        assert!(loc.is_some(), "expected LocatedError; got: {err:#}");
        let loc = loc.unwrap();
        assert!(loc.line >= 1, "{loc}");
    }

    #[test]
    fn rejects_invalid_spec_version() {
        let bad = r#"
spec_version = "2.0"

[project]
name = "demo"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#;
        let err = parse_str(bad).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("spec_version") || s.contains("pattern"), "{s}");
    }
}
