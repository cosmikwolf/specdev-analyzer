//! Read/write `specs/specdev/gen-lineage.toml` + the gen-drift report
//! (D64 in `02-decisions.md`).
//!
//! `lineage.toml` witnesses the spec-machinery direction (inlined plugin
//! content). This file witnesses the other direction: files emitted by
//! the `/spec-gen` skill. The analyzer cannot record at `gen` time — the
//! `gen` subcommand only assembles the codegen prompt, and which files
//! get written is decided by the profile prose at emission. Recording is
//! therefore a separate deterministic subcommand (`gen-record`) the skill
//! invokes after emission; `report gen-drift` later classifies each
//! recorded file as clean / edited / missing.
//!
//! Shape:
//!
//! ```toml
//! [files."src/dsp.rs"]
//! module       = "Dsp"            # absent for project-level files (Cargo.toml)
//! generated_at = "2026-06-12T10:00:00Z"
//! content_hash = "sha256:..."
//! ```
//!
//! Keys are project-root-relative paths with forward slashes. Re-recording
//! a path replaces its entry — the file always reflects the latest gen run.
//! Byte-stability via `BTreeMap`, same discipline as [`crate::lineage`].
//!
//! Scope fence (D64): this is a hash-level tripwire only. It detects THAT
//! a generated file changed, not WHAT changed or whether the change is
//! spec-equivalent. Semantic comparison is the (separately gated) audit
//! subcommand's territory.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::hash::content_hash;
use crate::layout::LayoutContext;

/// File name relative to `specs/specdev/`.
pub const GEN_LINEAGE_FILE: &str = "gen-lineage.toml";

/// In-memory gen lineage. Empty by default; updated by `gen-record`.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct GenLineage {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, GenEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenEntry {
    /// Module the file was generated for. Absent for project-level
    /// emissions (Cargo.toml).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub generated_at: String,
    pub content_hash: String,
}

impl GenLineage {
    /// Load from `<specdev_root>/gen-lineage.toml`. Returns an empty
    /// record if the file does not exist (no gen run recorded yet).
    pub fn load(specdev_root: &Path) -> Result<Self> {
        let path = gen_lineage_path(specdev_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).map_err(|e| {
            anyhow!(
                "[GEN-LINEAGE-001] {} is not a valid gen-lineage.toml: {}",
                path.display(),
                e
            )
        })
    }

    /// Save to `<specdev_root>/gen-lineage.toml`. Byte-stable across runs
    /// because the underlying map is `BTreeMap`.
    pub fn save(&self, specdev_root: &Path) -> Result<()> {
        let path = gen_lineage_path(specdev_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = self.to_toml_string()?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Serialize to a TOML string (byte-stable). Public for tests.
    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string(self).context("serializing gen-lineage")
    }
}

/// Absolute path of the gen-lineage file under a `specs/specdev/` root.
pub fn gen_lineage_path(specdev_root: &Path) -> PathBuf {
    specdev_root.join(GEN_LINEAGE_FILE)
}

/// Hash each file's current bytes and upsert its entry. Paths may be
/// absolute or relative to the current directory; each must resolve to a
/// file inside the project root. Returns the recorded project-root-relative
/// keys (sorted) for the caller's report.
pub fn record_files(
    layout: &LayoutContext,
    module: Option<&str>,
    files: &[PathBuf],
    generated_at: &str,
) -> Result<Vec<String>> {
    if files.is_empty() {
        bail!("gen-record: no files given — pass the emitted file paths");
    }
    let project_root = layout
        .project_root
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", layout.project_root.display()))?;

    let mut lineage = GenLineage::load(&layout.specdev_root)?;
    let mut recorded = Vec::with_capacity(files.len());
    for file in files {
        let abs = file
            .canonicalize()
            .with_context(|| format!("gen-record: {} does not exist", file.display()))?;
        let rel = abs.strip_prefix(&project_root).map_err(|_| {
            anyhow!(
                "gen-record: {} is outside the project root {}",
                abs.display(),
                project_root.display()
            )
        })?;
        let key = rel_key(rel);
        let bytes =
            std::fs::read(&abs).with_context(|| format!("reading {}", abs.display()))?;
        lineage.files.insert(
            key.clone(),
            GenEntry {
                module: module.map(str::to_string),
                generated_at: generated_at.to_string(),
                content_hash: content_hash(&bytes),
            },
        );
        recorded.push(key);
    }
    lineage.save(&layout.specdev_root)?;
    recorded.sort();
    Ok(recorded)
}

/// Project-root-relative key with forward slashes on every platform.
fn rel_key(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Per-file gen-drift classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenFileCategory {
    /// Current bytes hash to the recorded hash.
    Clean,
    /// File present but hash differs: generated-then-hand-edited.
    Edited,
    /// Recorded but no longer on disk.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenFileDrift {
    pub path: String,
    pub category: GenFileCategory,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub generated_at: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GenDriftReport {
    /// Sorted by path (BTreeMap iteration order) for stable snapshots.
    pub files: Vec<GenFileDrift>,
}

impl GenDriftReport {
    /// Render as canonical YAML for snapshot tests and skill consumption.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).context("serializing gen-drift report")
    }

    /// `true` iff every recorded file is `Clean`.
    pub fn is_clean(&self) -> bool {
        self.files
            .iter()
            .all(|f| f.category == GenFileCategory::Clean)
    }
}

/// Classify every recorded file against its current bytes. Pure reporter:
/// never modifies files or the record.
pub fn run_gen_drift(layout: &LayoutContext) -> Result<GenDriftReport> {
    let lineage = GenLineage::load(&layout.specdev_root)?;
    let mut report = GenDriftReport::default();
    for (path, entry) in &lineage.files {
        let abs = layout.project_root.join(path);
        let category = match std::fs::read(&abs) {
            Ok(bytes) => {
                if content_hash(&bytes) == entry.content_hash {
                    GenFileCategory::Clean
                } else {
                    GenFileCategory::Edited
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => GenFileCategory::Missing,
            Err(e) => {
                return Err(anyhow!("reading {}: {e}", abs.display()));
            }
        };
        report.files.push(GenFileDrift {
            path: path.clone(),
            category,
            module: entry.module.clone(),
            generated_at: entry.generated_at.clone(),
        });
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("specdev-gen-lineage-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tmp("missing");
        let l = GenLineage::load(&dir).unwrap();
        assert!(l.files.is_empty());
    }

    #[test]
    fn round_trip_preserves_shape() {
        let dir = tmp("roundtrip");
        let mut l = GenLineage::default();
        l.files.insert(
            "src/dsp.rs".into(),
            GenEntry {
                module: Some("Dsp".into()),
                generated_at: "2026-06-12T10:00:00Z".into(),
                content_hash: "sha256:abc123".into(),
            },
        );
        l.files.insert(
            "Cargo.toml".into(),
            GenEntry {
                module: None,
                generated_at: "2026-06-12T10:00:00Z".into(),
                content_hash: "sha256:def456".into(),
            },
        );
        l.save(&dir).unwrap();

        let l2 = GenLineage::load(&dir).unwrap();
        assert_eq!(l, l2);
    }

    #[test]
    fn output_is_byte_stable() {
        let mk = |order: &[(&str, &str)]| {
            let mut l = GenLineage::default();
            for (path, hash) in order {
                l.files.insert(
                    (*path).into(),
                    GenEntry {
                        module: None,
                        generated_at: "2026-06-12T10:00:00Z".into(),
                        content_hash: (*hash).into(),
                    },
                );
            }
            l.to_toml_string().unwrap()
        };
        let a = mk(&[("src/a.rs", "sha256:a"), ("src/z.rs", "sha256:z")]);
        let b = mk(&[("src/z.rs", "sha256:z"), ("src/a.rs", "sha256:a")]);
        assert_eq!(a, b);
    }

    #[test]
    fn malformed_toml_returns_gen_lineage_001() {
        let dir = tmp("malformed");
        fs::write(gen_lineage_path(&dir), "not = valid = toml").unwrap();
        let err = GenLineage::load(&dir).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("GEN-LINEAGE-001"), "{s}");
    }

    #[test]
    fn module_field_omitted_when_none() {
        let mut l = GenLineage::default();
        l.files.insert(
            "Cargo.toml".into(),
            GenEntry {
                module: None,
                generated_at: "2026-06-12T10:00:00Z".into(),
                content_hash: "sha256:abc".into(),
            },
        );
        let text = l.to_toml_string().unwrap();
        assert!(!text.contains("module"), "{text}");
    }
}
