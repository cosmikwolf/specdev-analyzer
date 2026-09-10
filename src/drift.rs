//! `specdev drift` — observational reporter for inlined units.
//!
//! For every unit recorded in `lineage.toml`, classify each file as one
//! of:
//!
//! - `Clean`         — `upstream == ours == plugin`
//! - `PluginMoved`   — `upstream != plugin` AND `ours == upstream`
//! - `LocallyEdited` — `ours != upstream` AND `plugin == upstream`
//! - `Both`          — `ours != upstream` AND `plugin != upstream`
//!
//! Pure reporter: drift never modifies files. The merge-with-base call is
//! `specdev inline --merge` ([`crate::merge`]).
//!
//! Three broken-invariant cases are reported as **errors**, not as
//! categories — they signal a corrupted project tree:
//!
//! - `DRIFT-INVARIANT-001` — inlined file present, no upstream entry
//! - `DRIFT-INVARIANT-002` — upstream entry present, no inlined file
//! - `DRIFT-INVARIANT-003` — lineage entry present, neither inlined nor upstream
//!
//! The report also covers the **profile substrate** (#109): the schemas,
//! templates, `profile.yaml` and root TOMLs that `bootstrap` scaffolded.
//! Those have no `upstream/` copy — their merge base is the hash
//! `bootstrap` recorded in `lineage.toml` `[profile_files]`, so the same
//! four categories are computable while a three-way *merge* is not. A tree
//! bootstrapped before #109 records no hashes; it reports
//! `status: not_recorded`, which is deliberately not `clean`. Reporting a
//! confident pass over a substrate the tool never looked at is the defect
//! #109 filed: `drift` said clean on `dogfood-flutter-hello/rust` while a
//! stale inlined `architecture.schema.yaml` was a hard `SCHEMA-ARCH`
//! error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::bootstrap::{self, BootstrapInputs};
use crate::inline::{collect_block_files, collect_bridge_files, UnitFile};
use crate::layout::LayoutContext;
use crate::lineage::Lineage;
use crate::upstream::{self, UnitKind};

/// Per-file drift classification. `Clean` is the desired state; the
/// others are real divergence the user should investigate or merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FileCategory {
    Clean,
    PluginMoved,
    LocallyEdited,
    Both,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileDrift {
    /// Path under `specs/specdev/upstream/<unit-kind>/<name>/`. Identifies
    /// the file inside the unit (e.g. `schema.yaml`, `codegen/rust/README.md`).
    pub rel_in_unit: PathBuf,
    pub category: FileCategory,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnitDrift {
    pub kind: String,
    pub name: String,
    /// Sorted by `rel_in_unit` for stable snapshots.
    pub files: Vec<FileDrift>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvariantViolation {
    pub code: String,
    pub kind: String,
    pub name: String,
    pub message: String,
}

/// How much of a file the category below is a statement about. Stated in
/// the report rather than left as a rule the reader has to know: a partial
/// comparison that looks total is how a confident clean gets published
/// over a substrate nobody checked (#109).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
    /// Whole file.
    Full,
    /// Everything except the `allOf` array that `inline` appends to. That
    /// array's contents are reported by `units:`.
    ExcludesUnitRefs,
}

/// One profile-scaffolded file, classified against the hash `bootstrap`
/// recorded for it.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileFileDrift {
    /// Path relative to the PROJECT ROOT, `/`-separated — not relative to
    /// a unit directory. A profile writes to `spec.toml`, `specs/*.toml`
    /// and `specs/specdev/**`, so there is no single containing directory
    /// to be relative to.
    pub path: String,
    pub category: FileCategory,
    pub comparison: Comparison,
}

/// Whether this project records the substrate at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileDriftStatus {
    /// `[profile_files]` is populated; the categories below are real.
    Recorded,
    /// Bootstrapped before #109. Nothing to compare against, so nothing
    /// is asserted — in particular, NOT that the substrate is clean.
    NotRecorded,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileDrift {
    /// `[project].profile` from `spec.toml`.
    pub profile: String,
    pub status: ProfileDriftStatus,
    /// Sorted by `path`. Empty when `status` is `not_recorded`.
    pub files: Vec<ProfileFileDrift>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct DriftReport {
    /// Sorted by `(kind, name)` for stable snapshots.
    pub units: Vec<UnitDrift>,
    /// Broken-invariant errors. Sorted by `(code, kind, name)`.
    pub invariants: Vec<InvariantViolation>,
    /// The profile substrate (#109). `None` only when the project has no
    /// resolvable profile directory in the plugin tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ProfileDrift>,
}

impl DriftReport {
    /// Render as canonical YAML (BTreeMap-backed) for snapshot tests.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).context("serializing drift report")
    }

    /// `true` iff every file is `Clean` AND there are no invariant errors.
    ///
    /// A `not_recorded` profile substrate does NOT make this false. The
    /// tree is not dirty; the tool simply cannot see that far, and every
    /// project bootstrapped before #109 is in that state. Callers that
    /// need to know should ask [`Self::profile_substrate_unrecorded`] —
    /// silence and cleanliness are different answers and get different
    /// questions.
    pub fn is_clean(&self) -> bool {
        self.invariants.is_empty()
            && self
                .units
                .iter()
                .all(|u| u.files.iter().all(|f| f.category == FileCategory::Clean))
            && self.profile.as_ref().is_none_or(|p| {
                p.files.iter().all(|f| f.category == FileCategory::Clean)
            })
    }

    /// `true` when the project predates `[profile_files]`, so no claim
    /// about the profile substrate is being made either way.
    pub fn profile_substrate_unrecorded(&self) -> bool {
        self.profile
            .as_ref()
            .is_some_and(|p| p.status == ProfileDriftStatus::NotRecorded)
    }
}

pub fn run_drift(layout: &LayoutContext, plugin_root: &Path) -> Result<DriftReport> {
    let lineage = Lineage::load(&layout.specdev_root)?;
    let mut report = DriftReport::default();

    let mut entries: Vec<(UnitKind, &str)> = Vec::new();
    for name in lineage.blocks.keys() {
        entries.push((UnitKind::Block, name.as_str()));
    }
    for name in lineage.bridges.keys() {
        entries.push((UnitKind::Bridge, name.as_str()));
    }
    entries.sort_by(|a, b| match (a.0, b.0) {
        (UnitKind::Block, UnitKind::Bridge) => std::cmp::Ordering::Less,
        (UnitKind::Bridge, UnitKind::Block) => std::cmp::Ordering::Greater,
        _ => a.1.cmp(b.1),
    });

    for (kind, name) in entries {
        match drift_one_unit(layout, plugin_root, kind, name) {
            Ok(unit_drift) => report.units.push(unit_drift),
            Err(violation) => report.invariants.push(violation),
        }
    }

    report.profile = drift_profile(layout, plugin_root, &lineage, &mut report.invariants)?;

    report
        .invariants
        .sort_by(|a, b| (&a.code, &a.kind, &a.name).cmp(&(&b.code, &b.kind, &b.name)));
    Ok(report)
}

/// Classify the profile substrate (#109).
///
/// The three values compared per file:
///
/// * **base** — the hash `bootstrap` recorded in `[profile_files]`.
/// * **ours** — the file on disk in the project now.
/// * **theirs** — what the CURRENT plugin would write for that path,
///   recomputed through [`bootstrap::profile_expected_bytes`] with the
///   same substitution inputs. Recomputing rather than reading the raw
///   template is the whole trick: `spec.toml` and `module.schema.yaml`
///   carry `{{project_name}}` / `{{language}}`, and comparing against
///   template text would report every templated file as drifted forever.
///
/// The substitution inputs need no new stored state — `spec.toml`'s
/// `[project]` table already carries `name`, `profile` and `language`,
/// which is exactly what `bootstrap` was given.
fn drift_profile(
    layout: &LayoutContext,
    plugin_root: &Path,
    lineage: &Lineage,
    invariants: &mut Vec<InvariantViolation>,
) -> Result<Option<ProfileDrift>> {
    let project = &layout.spec.project;
    let profile_dir = plugin_root.join("profiles").join(&project.profile);
    if !profile_dir.is_dir() {
        // The plugin has no such profile. `migrate` reports this as
        // MIGRATE-002; drift stays a pure reporter and says nothing.
        return Ok(None);
    }

    if !lineage.records_profile_files() {
        return Ok(Some(ProfileDrift {
            profile: project.profile.clone(),
            status: ProfileDriftStatus::NotRecorded,
            files: Vec::new(),
        }));
    }

    let inputs = BootstrapInputs {
        profile: project.profile.clone(),
        language: project.language.clone(),
        project_name: project.name.clone(),
        target_dir: layout.project_root.clone(),
    };
    let theirs = bootstrap::profile_expected_bytes(&profile_dir, &inputs)?;

    // Union of what bootstrap recorded and what the plugin ships now, so
    // a file the plugin added or dropped since bootstrap still appears.
    let mut paths: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    paths.extend(lineage.profile_files.keys().map(|k| k.as_str()));
    paths.extend(theirs.keys().map(|k| k.as_str()));

    let mut files: Vec<ProfileFileDrift> = Vec::new();
    for path in paths {
        let base = lineage.profile_files.get(path).map(|s| s.as_str());
        let theirs_hash = match theirs.get(path) {
            Some(b) => Some(bootstrap::profile_file_hash(path, b)?),
            None => None,
        };
        let abs = layout.project_root.join(path);
        let ours_hash = match std::fs::read(&abs) {
            Ok(b) => Some(bootstrap::profile_file_hash(path, &b)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(anyhow::Error::new(e).context(format!("reading {}", abs.display())))
            }
        };

        // Recorded, but gone from the project: the same shape as an
        // inlined unit whose file was deleted, so it reuses that code.
        if base.is_some() && ours_hash.is_none() {
            invariants.push(violation(
                "DRIFT-INVARIANT-002",
                "profile",
                &project.profile,
                format!("`{path}` is recorded in [profile_files] but is not in the project"),
            ));
            continue;
        }
        // Present but never recorded: bootstrap did not write it. Either
        // hand-added, or hand-copied from a newer plugin.
        if base.is_none() {
            invariants.push(violation(
                "DRIFT-INVARIANT-001",
                "profile",
                &project.profile,
                format!(
                    "`{path}` is shipped by the profile but has no [profile_files] record; \
                     bootstrap did not write it"
                ),
            ));
            continue;
        }

        let base = base.expect("base checked above");
        let ours_moved = ours_hash.as_deref() != Some(base);
        // A path the plugin no longer produces counts as moved: the
        // plugin side genuinely differs from the base.
        let theirs_moved = theirs_hash.as_deref() != Some(base);
        let category = match (ours_moved, theirs_moved) {
            (false, false) => FileCategory::Clean,
            (false, true) => FileCategory::PluginMoved,
            (true, false) => FileCategory::LocallyEdited,
            (true, true) => FileCategory::Both,
        };
        files.push(ProfileFileDrift {
            path: path.to_string(),
            category,
            comparison: if bootstrap::CO_OWNED_AGGREGATORS.contains(&path) {
                Comparison::ExcludesUnitRefs
            } else {
                Comparison::Full
            },
        });
    }

    Ok(Some(ProfileDrift {
        profile: project.profile.clone(),
        status: ProfileDriftStatus::Recorded,
        files,
    }))
}

/// Classify one unit's file set. Returns `Err(InvariantViolation)` for
/// broken-invariant cases (DRIFT-INVARIANT-001..003).
fn drift_one_unit(
    layout: &LayoutContext,
    plugin_root: &Path,
    kind: UnitKind,
    name: &str,
) -> std::result::Result<UnitDrift, InvariantViolation> {
    let kind_word = match kind {
        UnitKind::Block => "block",
        UnitKind::Bridge => "bridge",
    };

    // Read upstream bytes (the merge base recorded at inline time).
    let upstream_files = upstream::read_upstream_unit(&layout.specdev_root, kind, name)
        .map_err(|e| violation("DRIFT-IO", kind_word, name, format!("{e:#}")))?;

    // Re-fetch plugin (theirs).
    let plugin_files: Vec<UnitFile> = match kind {
        UnitKind::Block => collect_block_files(plugin_root, name),
        UnitKind::Bridge => collect_bridge_files(plugin_root, name),
    }
    .map_err(|e| violation("DRIFT-IO", kind_word, name, format!("{e:#}")))?;
    let plugin_index: BTreeMap<PathBuf, &UnitFile> = plugin_files
        .iter()
        .map(|f| (f.rel_src_in_unit.clone(), f))
        .collect();

    // Both upstream AND plugin are empty: the lineage entry points at
    // nothing on either side. DRIFT-INVARIANT-003.
    if upstream_files.is_empty() && plugin_index.is_empty() {
        return Err(violation(
            "DRIFT-INVARIANT-003",
            kind_word,
            name,
            "lineage entry present, but no files in upstream/ and the plugin has no such unit (orphaned lineage)".to_string(),
        ));
    }

    // Iterate over the union of upstream and plugin file keys.
    let mut keys: std::collections::BTreeSet<PathBuf> =
        std::collections::BTreeSet::new();
    keys.extend(upstream_files.keys().cloned());
    keys.extend(plugin_index.keys().cloned());

    let mut files: Vec<FileDrift> = Vec::new();
    for rel_in_unit in keys {
        // Resolve the project-destination path. Prefer the plugin's
        // collect (which carries rel_dst); fall back to a synthesized
        // path from rel_in_unit when the file only exists in upstream
        // (plugin removed it).
        let project_path =
            project_path_for(layout, kind, name, &rel_in_unit, plugin_index.get(&rel_in_unit));

        let project_bytes_opt = match std::fs::read(&project_path) {
            Ok(b) => Some(b),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(violation(
                    "DRIFT-IO",
                    kind_word,
                    name,
                    format!("reading {}: {e}", project_path.display()),
                ));
            }
        };

        let upstream_bytes = upstream_files.get(&rel_in_unit);
        let plugin_bytes = plugin_index.get(&rel_in_unit).map(|f| &f.bytes);

        let category = match (upstream_bytes, project_bytes_opt.as_ref(), plugin_bytes) {
            // All three present — standard 3-way classification.
            (Some(base), Some(ours), Some(theirs)) => classify(base, ours, theirs),
            // Project file missing while upstream still records it: invariant violation.
            (Some(_), None, _) => {
                return Err(violation(
                    "DRIFT-INVARIANT-002",
                    kind_word,
                    name,
                    format!(
                        "upstream entry present for {} but no inlined file at {}",
                        rel_in_unit.display(),
                        project_path.display(),
                    ),
                ));
            }
            // Project file present but no upstream entry: invariant violation.
            (None, Some(_), _) => {
                return Err(violation(
                    "DRIFT-INVARIANT-001",
                    kind_word,
                    name,
                    format!(
                        "inlined file at {} present but no upstream entry for {}",
                        project_path.display(),
                        rel_in_unit.display(),
                    ),
                ));
            }
            // Plugin-only (upstream + project both missing): the plugin
            // added a file since inlining. v1 doesn't auto-import;
            // surface as PluginMoved on a synthesized empty base. Future
            // versions can widen.
            (None, None, Some(_)) => FileCategory::PluginMoved,
            // Plugin removed a file the project still has. Project may
            // also have local edits. Classify as the closest 3-way fit:
            // base == ours → PluginMoved (plugin removed = plugin moved);
            // base != ours → Both (plugin removed AND ours edited).
            (Some(base), Some(ours), None) => {
                if base == ours {
                    FileCategory::PluginMoved
                } else {
                    FileCategory::Both
                }
            }
            // None of the three present — should be filtered by the
            // earlier empty-set check, but defend.
            (None, None, None) => continue,
        };

        files.push(FileDrift {
            rel_in_unit,
            category,
        });
    }

    files.sort_by(|a, b| a.rel_in_unit.cmp(&b.rel_in_unit));
    Ok(UnitDrift {
        kind: kind_word.to_string(),
        name: name.to_string(),
        files,
    })
}

fn classify(base: &[u8], ours: &[u8], theirs: &[u8]) -> FileCategory {
    let plugin_moved = theirs != base;
    let locally_edited = ours != base;
    match (plugin_moved, locally_edited) {
        (false, false) => FileCategory::Clean,
        (true, false) => FileCategory::PluginMoved,
        (false, true) => FileCategory::LocallyEdited,
        (true, true) => FileCategory::Both,
    }
}

/// Resolve the project-destination path for a unit-relative file. When
/// the plugin has the file (and therefore a `UnitFile` with `rel_dst`),
/// use that; otherwise synthesize from the rel_in_unit + unit kind/name.
fn project_path_for(
    layout: &LayoutContext,
    kind: UnitKind,
    name: &str,
    rel_in_unit: &Path,
    plugin_file: Option<&&UnitFile>,
) -> PathBuf {
    if let Some(f) = plugin_file {
        return layout.specdev_root.join(&f.rel_dst);
    }
    // Synthesized — best-effort guess for files the plugin no longer has.
    // Match the pattern collect_*_files would have produced.
    let s = rel_in_unit.to_string_lossy();
    let suffix = if s == "schema.yaml" || s == "bindings.schema.yaml" {
        match kind {
            UnitKind::Block => format!("schemas/inlined/blocks/{name}.schema.yaml"),
            UnitKind::Bridge => format!("schemas/inlined/bridges/{name}.schema.yaml"),
        }
    } else if s == "reduction.yaml" {
        match kind {
            UnitKind::Block => format!("reductions/blocks/{name}.yaml"),
            UnitKind::Bridge => format!("reductions/bridges/{name}.yaml"),
        }
    } else if let Some(rest) = s.strip_prefix("codegen/") {
        let parts: Vec<&str> = rest.splitn(2, '/').collect();
        if parts.len() == 2 && parts[1] == "README.md" {
            let lang = parts[0];
            match kind {
                UnitKind::Block => format!("codegen/{lang}/blocks/{name}/README.md"),
                UnitKind::Bridge => format!("codegen/{lang}/bridges/{name}/README.md"),
            }
        } else {
            s.into_owned()
        }
    } else {
        s.into_owned()
    };
    layout.specdev_root.join(suffix)
}

fn violation(code: &str, kind: &str, name: &str, message: String) -> InvariantViolation {
    InvariantViolation {
        code: code.to_string(),
        kind: kind.to_string(),
        name: name.to_string(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_clean_when_all_match() {
        assert_eq!(classify(b"x", b"x", b"x"), FileCategory::Clean);
    }

    #[test]
    fn classify_plugin_moved_when_only_theirs_differs() {
        assert_eq!(classify(b"x", b"x", b"y"), FileCategory::PluginMoved);
    }

    #[test]
    fn classify_locally_edited_when_only_ours_differs() {
        assert_eq!(classify(b"x", b"y", b"x"), FileCategory::LocallyEdited);
    }

    #[test]
    fn classify_both_when_ours_and_theirs_differ() {
        assert_eq!(classify(b"x", b"y", b"z"), FileCategory::Both);
        // Identical edits on both sides: still classified Both since
        // ours != base AND theirs != base. (Merge would resolve cleanly,
        // but drift's job is observation, not resolution.)
        assert_eq!(classify(b"x", b"y", b"y"), FileCategory::Both);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(128))]

        /// Property: classify maps strictly per the (base==ours, base==theirs)
        /// truth table. Random byte triples cover patterns the four hand-
        /// picked unit tests can't enumerate (empty bytes, identical
        /// prefixes with different suffixes, very large buffers, equal
        /// edits on both sides, etc.).
        #[test]
        fn classify_truth_table(
            base in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256),
            ours in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256),
            theirs in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256),
        ) {
            let category = classify(&base, &ours, &theirs);
            let plugin_moved = base != theirs;
            let locally_edited = base != ours;
            let expected = match (plugin_moved, locally_edited) {
                (false, false) => FileCategory::Clean,
                (true, false) => FileCategory::PluginMoved,
                (false, true) => FileCategory::LocallyEdited,
                (true, true) => FileCategory::Both,
            };
            proptest::prop_assert_eq!(category, expected);
        }

        /// Property: classify(b, b, b) is always Clean for any b.
        /// Sanity-check: the "all three equal" branch is the only path
        /// to Clean.
        #[test]
        fn classify_clean_iff_all_equal(
            v in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256),
        ) {
            proptest::prop_assert_eq!(classify(&v, &v, &v), FileCategory::Clean);
        }
    }
}
