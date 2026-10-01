//! Read/write `specs/specdev/lineage.toml`.
//!
//! Shape:
//!
//! ```toml
//! [meta]
//! bootstrapped_with = "0.4.2"   # `plugin_version` in a 1.x tree
//! bootstrapped_at = "2026-04-28T10:00:00Z"
//! profile         = "firmware-layered"
//!
//! [tooling]
//! analyzer_version = "0.2.0"
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
//!
//! [profile_files]
//! "spec.toml" = "sha256:..."
//! "specs/specdev/schemas/module.schema.yaml" = "sha256:..."
//! ```
//!
//! The single `content_hash` per unit is a rolled-up hash over the unit's
//! file set (see [`crate::inline`]).
//!
//! `[profile_files]` records every file `bootstrap` scaffolded from the
//! profile, keyed by path relative to the project root, hashed **as
//! written** — post-substitution, because `spec.toml` and
//! `module.schema.yaml` carry `{{project_name}}` / `{{language}}` tokens
//! and hashing the plugin's template text would report drift forever.
//!
//! This overturns a premise (#109). Earlier revisions said profiles
//! "intentionally have no entry — they are one-shot scaffolding", which was
//! true when a profile shipped templates and prose. A profile now ships the
//! project's *schemas* and its `profile.yaml` manifest, and every analyzer
//! command reads them. Measured on `dogfood-flutter-hello/rust`: `drift`
//! reported clean while the inlined `architecture.schema.yaml` was missing
//! `external_deps` — a hard `SCHEMA-ARCH` error that blocked
//! `manifest --write`. A substrate nothing hashes is a substrate nothing
//! can report on.
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
    /// Which analyzer last wrote this file. Refreshed on every write; the
    /// stamp lives in [`Lineage::save`], not here and not in `[meta]`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooling: Option<ToolingSection>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub blocks: BTreeMap<String, Entry>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub bridges: BTreeMap<String, Entry>,
    /// Profile-scaffolded files: project-relative path → hash of the bytes
    /// bootstrap wrote. Empty for every tree bootstrapped before #109; an
    /// empty map is "unknown", never "clean", and `drift` says so rather
    /// than reporting a confident pass over a substrate it cannot see.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profile_files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetaSection {
    /// The plugin version at bootstrap. Never advances.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrapped_with: Option<String>,
    /// The `1.x` name of [`Self::bootstrapped_with`]. Read, and kept as it
    /// is, in a `1.x` tree; `migrate` renames it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_version: Option<String>,
    pub bootstrapped_at: String,
    pub profile: String,
}

/// Tooling provenance. Deliberately **not** a field of [`MetaSection`]:
/// `[meta]` is frozen at bootstrap (`bootstrapped_at` never moves again),
/// and a live field beside a frozen one is one table with two lifetimes —
/// a shape that has produced defects in this file before. A reader can
/// therefore trust every key in `[meta]` to describe the bootstrap, and
/// every key here to describe the last write.
impl MetaSection {
    /// The plugin version at bootstrap, under either name.
    pub fn bootstrapped_with(&self) -> &str {
        self.bootstrapped_with.as_deref().or(self.plugin_version.as_deref()).unwrap_or("")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolingSection {
    /// `CARGO_PKG_VERSION` of the analyzer that performed the last write.
    pub analyzer_version: String,
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
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
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
        // Stamp here, not in `to_toml_string()`: that method is kept pure so
        // the byte-stability test can compare two serializations of the same
        // value. `save()` is the single funnel every writer already goes
        // through (`bootstrap`, `inline` x2, `merge`, `migrate`), so a future
        // sixth writer inherits the stamp with nothing to remember.
        let mut stamped = self.clone();
        stamped.tooling = Some(ToolingSection {
            analyzer_version: env!("CARGO_PKG_VERSION").to_string(),
        });
        let text = stamped.to_toml_string()?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Serialize to a TOML string (byte-stable). Public for tests + the
    /// snapshot-stability check.
    pub fn to_toml_string(&self) -> Result<String> {
        toml::to_string(self).context("serializing lineage")
    }

    pub fn has_block(&self, name: &str) -> bool {
        self.blocks.contains_key(name)
    }

    pub fn has_bridge(&self, name: &str) -> bool {
        self.bridges.contains_key(name)
    }

    /// True when this tree records its profile substrate. False for every
    /// project bootstrapped before #109 — the caller must report "not
    /// recorded", not "clean".
    pub fn records_profile_files(&self) -> bool {
        !self.profile_files.is_empty()
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
        let mut l = Lineage {
            meta: Some(MetaSection {
                bootstrapped_with: Some("0.4.2".into()),
                plugin_version: None,
                bootstrapped_at: "2026-04-28T10:00:00Z".into(),
                profile: "firmware-layered".into(),
            }),
            ..Default::default()
        };
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
        // `save()` stamps `[tooling]`, so save -> load is deliberately NOT
        // the identity on that one field. Assert the stamp landed, then
        // compare the rest.
        assert_eq!(
            l2.tooling.as_ref().map(|t| t.analyzer_version.as_str()),
            Some(env!("CARGO_PKG_VERSION")),
            "save() must stamp the running analyzer's version"
        );
        let mut l2_rest = l2;
        l2_rest.tooling = l.tooling.clone();
        assert_eq!(l, l2_rest);
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

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// Property: `Lineage::load(save(x))` returns x for any
        /// well-formed Lineage. Exercises arbitrary block names, bridge
        /// names (containing `+`), origin URLs, version strings,
        /// timestamps, and content_hashes — many more shapes than the
        /// hand-built `round_trip_preserves_shape` test covers.
        #[test]
        fn save_load_round_trip(lineage in lineage_strategy()) {
            let dir = std::env::temp_dir().join(format!(
                "specdev-lineage-prop-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            lineage.save(&dir).unwrap();
            let loaded = Lineage::load(&dir).unwrap();
            let _ = fs::remove_dir_all(&dir);
            // Every field round-trips unchanged except `[tooling]`, which
            // `save()` overwrites with the running analyzer's version -
            // whatever the input carried. Check that first, because it is
            // the one field where equality would be the bug.
            proptest::prop_assert_eq!(
                loaded.tooling.as_ref().map(|t| t.analyzer_version.as_str()),
                Some(env!("CARGO_PKG_VERSION"))
            );
            let mut loaded_rest = loaded;
            loaded_rest.tooling = lineage.tooling.clone();
            proptest::prop_assert_eq!(loaded_rest, lineage);
        }
    }

    fn lineage_strategy() -> impl proptest::strategy::Strategy<Value = Lineage> {
        use proptest::collection::btree_map;
        use proptest::option;
        use proptest::strategy::Strategy;
        // Block names: identifier-shaped (matches real-world bootstrap output).
        let block_name = "[a-z][a-z0-9_-]{0,12}";
        // Bridge names: `<a>+<b>` with `a < b` lex-ordered. We don't
        // enforce ordering in the strategy because lineage save/load
        // doesn't care about lex order — that's `inline-bridge`'s job.
        let bridge_name = "[a-z][a-z0-9_-]{0,8}\\+[a-z][a-z0-9_-]{0,8}";

        // Profile-file keys are project-relative paths with `/`
        // separators (#109). Nested paths matter here: `[profile_files]`
        // keys contain dots and slashes, which TOML has to quote on the
        // way out and unquote on the way back in.
        let profile_path = "[a-z][a-z0-9_./-]{0,24}";
        let hash = "sha256:[0-9a-f]{64}";

        let blocks = btree_map(block_name, entry_strategy(), 0..6);
        let bridges = btree_map(bridge_name, entry_strategy(), 0..4);
        let profile_files = btree_map(profile_path, hash, 0..8);
        let meta = option::of(meta_strategy());
        // `[tooling]` is generated too, not pinned to None: the round-trip
        // property is the only thing that would catch a serde attribute on
        // the new table that silently drops it on the way back in.
        let tooling = option::of(
            "[0-9]{1,2}\\.[0-9]{1,2}\\.[0-9]{1,2}(-[a-z]{1,8})?"
                .prop_map(|analyzer_version| ToolingSection { analyzer_version }),
        );

        (meta, tooling, blocks, bridges, profile_files).prop_map(
            |(meta, tooling, blocks, bridges, profile_files)| Lineage {
                meta,
                tooling,
                blocks,
                bridges,
                profile_files,
            },
        )
    }

    fn entry_strategy() -> impl proptest::strategy::Strategy<Value = Entry> {
        use proptest::strategy::Strategy;
        // Restrict strings to ASCII-printable (excluding TOML's escape-
        // sensitive `"` `\` and control chars). The `toml` crate handles
        // wider ranges, but we avoid edges we don't care about for v1.
        let printable = "[a-zA-Z0-9_./:-]+";
        let hash = "sha256:[0-9a-f]{64}";
        let timestamp = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z";
        (printable, printable, timestamp, hash).prop_map(
            |(origin, origin_version, inlined_at, content_hash)| Entry {
                origin,
                origin_version,
                inlined_at,
                content_hash,
            },
        )
    }

    fn meta_strategy() -> impl proptest::strategy::Strategy<Value = MetaSection> {
        use proptest::strategy::Strategy;
        let printable = "[a-zA-Z0-9_./:-]+";
        let timestamp = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z";
        (printable, timestamp, printable).prop_map(|(version, bootstrapped_at, profile)| {
            MetaSection {
                bootstrapped_with: Some(version),
                plugin_version: None,
                bootstrapped_at,
                profile,
            }
        })
    }
}
