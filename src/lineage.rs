//! Read/write `specs/specdev/lineage.toml`.
//!
//! Shape (doc 15 §5.5; D33 in `02-decisions.md`):
//!
//! ```toml
//! [meta]
//! plugin_version  = "0.4.2"
//! bootstrapped_at = "2026-04-28T10:00:00Z"
//! profile         = "firmware-layered"
//!
//! [blocks.embassy]
//! origin         = "plugin"
//! origin_version = "0.4.2"
//! inlined_at     = "2026-04-28T10:14:00Z"
//! content_hash   = "sha256:..."
//!
//! [bridges."embassy+lvgl"]
//! origin         = "plugin"
//! origin_version = "0.4.2"
//! inlined_at     = "2026-04-28T10:15:00Z"
//! content_hash   = "sha256:..."
//! ```
//!
//! The single `content_hash` per unit is a rolled-up hash over the unit's
//! file set (see [`crate::inline`]). Profiles intentionally have no entry —
//! they are one-shot scaffolding (doc 15 §3.3).
//!
//! Byte-stability is preserved via `BTreeMap`: blocks and bridges serialize
//! in lex order regardless of insertion order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// File name relative to `specs/specdev/`.
pub const LINEAGE_FILE: &str = "lineage.toml";

/// In-memory lineage. Empty by default; constructed by `bootstrap` and
/// updated by `inline` / `inline-bridge`.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Lineage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<MetaSection>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub blocks: BTreeMap<String, Entry>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bridges: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetaSection {
    pub plugin_version: String,
    pub bootstrapped_at: String,
    pub profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub origin: String,
    pub origin_version: String,
    pub inlined_at: String,
    pub content_hash: String,
}

impl Lineage {
    /// Load lineage from `<specdev_root>/lineage.toml`. Returns an empty
    /// lineage if the file does not exist (a fresh project, pre-bootstrap).
    pub fn load(specdev_root: &Path) -> Result<Self> {
        let path = lineage_path(specdev_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text).map_err(|e| {
            anyhow!(
                "[LINEAGE-001] {} is not a valid lineage.toml: {}",
                path.display(),
                e
            )
        })
    }

    /// Save lineage to `<specdev_root>/lineage.toml`. Output is byte-stable
    /// across runs because the underlying maps are `BTreeMap`.
    pub fn save(&self, specdev_root: &Path) -> Result<()> {
        let path = lineage_path(specdev_root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = self.to_toml_string()?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Serialize to a TOML string (byte-stable). Public for tests + the
    /// snapshot-stability check in `tests/phase3.rs`.
    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string(self).context("serializing lineage")
    }

    pub fn has_block(&self, name: &str) -> bool {
        self.blocks.contains_key(name)
    }

    pub fn has_bridge(&self, name: &str) -> bool {
        self.bridges.contains_key(name)
    }
}

/// Absolute path of the lineage file under a `specs/specdev/` root.
pub fn lineage_path(specdev_root: &Path) -> PathBuf {
    specdev_root.join(LINEAGE_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("specdev-lineage-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn missing_file_loads_as_empty() {
        let dir = tmp("missing");
        let l = Lineage::load(&dir).unwrap();
        assert!(l.meta.is_none());
        assert!(l.blocks.is_empty());
        assert!(l.bridges.is_empty());
    }

    #[test]
    fn round_trip_preserves_shape() {
        let dir = tmp("roundtrip");
        let mut l = Lineage::default();
        l.meta = Some(MetaSection {
            plugin_version: "0.4.2".into(),
            bootstrapped_at: "2026-04-28T10:00:00Z".into(),
            profile: "firmware-layered".into(),
        });
        l.blocks.insert(
            "embassy".into(),
            Entry {
                origin: "plugin".into(),
                origin_version: "0.4.2".into(),
                inlined_at: "2026-04-28T10:14:00Z".into(),
                content_hash: "sha256:abc123".into(),
            },
        );
        l.bridges.insert(
            "embassy+lvgl".into(),
            Entry {
                origin: "plugin".into(),
                origin_version: "0.4.2".into(),
                inlined_at: "2026-04-28T10:15:00Z".into(),
                content_hash: "sha256:def456".into(),
            },
        );
        l.save(&dir).unwrap();

        let l2 = Lineage::load(&dir).unwrap();
        assert_eq!(l, l2);
    }

    #[test]
    fn output_is_byte_stable() {
        // Build the lineage twice with insertions in different orders;
        // because of BTreeMap, both serializations should be byte-identical.
        let mk = |order: &[(&str, &str)]| {
            let mut l = Lineage::default();
            for (name, hash) in order {
                l.blocks.insert(
                    (*name).into(),
                    Entry {
                        origin: "plugin".into(),
                        origin_version: "0.4.2".into(),
                        inlined_at: "2026-04-28T10:14:00Z".into(),
                        content_hash: (*hash).into(),
                    },
                );
            }
            l.to_toml_string().unwrap()
        };
        let a = mk(&[("alpha", "sha256:a"), ("zulu", "sha256:z")]);
        let b = mk(&[("zulu", "sha256:z"), ("alpha", "sha256:a")]);
        assert_eq!(a, b);
    }

    #[test]
    fn malformed_toml_returns_lineage_001() {
        let dir = tmp("malformed");
        fs::write(lineage_path(&dir), "not = valid = toml").unwrap();
        let err = Lineage::load(&dir).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("LINEAGE-001"), "{s}");
    }
}
