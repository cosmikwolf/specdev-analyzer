//! `specdev-analyzer migrate` — populate Phase 16 manifests for pre-D52 projects.
//!
//! Doc 21 §6.5 specifies the backwards-compat path: projects bootstrapped
//! before Phase 16 ship no `profile.yaml` / `deps.yaml` / `contract.yaml` in
//! their inlined tree. `migrate` walks the project's lineage, resolves which
//! profile / blocks / bridges it has, and byte-copies the missing manifests
//! from the plugin tree's HEAD. Lineage `content_hash` is updated to the
//! new (post-manifest) rolled-up hash; `origin` / `origin_version` /
//! `inlined_at` stay frozen — they record when the unit was *originally*
//! inlined, not when it was migrated.
//!
//! Idempotency: when every lineage entry's hash already matches the plugin-
//! HEAD rolled-up hash, migrate is a true no-op (zero file writes). When
//! the plugin tree itself is pre-D52 (no manifests), every collector
//! returns the legacy file set and the rolled-up hash matches lineage —
//! migrate becomes a no-op for those units. This is correct: there is
//! nothing to migrate to.
//!
//! Project-local bridges (`reductions/bridges/local/...`) have no lineage
//! entry per D33 and are skipped — there is no plugin source to byte-copy
//! from.
//!
//! Error families:
//!
//! - `MIGRATE-001` — project is pre-bootstrap (lineage `[meta]` is None).
//!   Migrate refuses cleanly; the user must `bootstrap` first.
//! - `MIGRATE-002` — a lineage block/bridge name is not present in the
//!   plugin tree (e.g. user-deleted block). Migrate surfaces the missing
//!   path on stderr, continues with the remaining units, and exits non-
//!   zero. The unit's lineage entry is left untouched.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::inline::{
    collect_block_files, collect_bridge_files, rollup_hash, write_unit_files, UnitFile,
};
use crate::layout::LayoutContext;
use crate::lineage::Lineage;
use crate::upstream::{self, UnitKind};

/// One unit migrated (or planned, in dry-run mode).
#[derive(Debug, Clone)]
pub struct MigratedUnit {
    pub kind: UnitKind,
    pub name: String,
    /// Pre-migrate rolled-up hash from the lineage entry.
    pub before_hash: String,
    /// Post-migrate rolled-up hash from the plugin-HEAD file set.
    pub after_hash: String,
    /// Files written under `specdev_root` (relative paths). One per
    /// `UnitFile` whose destination did not exist before this migrate.
    pub files_written: Vec<PathBuf>,
}

/// One unit skipped because the lineage hash already matches plugin HEAD.
#[derive(Debug, Clone)]
pub struct SkippedUnit {
    pub kind: UnitKind,
    pub name: String,
    pub hash: String,
}

/// Plan + apply summary returned by [`run_migrate`].
#[derive(Debug, Clone, Default)]
pub struct MigrateReport {
    /// Profile-shell migration (one entry max — there is at most one
    /// inlined profile per project per doc 15 §3.3).
    pub profile_files_written: Vec<PathBuf>,
    pub profile_skipped: bool,
    pub migrated: Vec<MigratedUnit>,
    pub skipped: Vec<SkippedUnit>,
    pub missing_in_plugin: Vec<String>,
    pub dry_run: bool,
}

impl MigrateReport {
    /// Total file-write count across every per-unit migration plus the
    /// profile shell.
    pub fn total_files_written(&self) -> usize {
        self.profile_files_written.len()
            + self.migrated.iter().map(|m| m.files_written.len()).sum::<usize>()
    }
}

/// Run `specdev-analyzer migrate`. The CLI calls this; the function
/// performs detection, planning, and (when `dry_run == false`) the file
/// writes + lineage update.
pub fn run_migrate(
    layout: &LayoutContext,
    plugin_root: &Path,
    dry_run: bool,
) -> Result<MigrateReport> {
    let mut lineage = Lineage::load(&layout.specdev_root)?;
    let meta = lineage.meta.as_ref().ok_or_else(|| {
        anyhow!(
            "[MIGRATE-001] {} has no [meta] lineage stamp; project is pre-bootstrap. Run `specdev-analyzer bootstrap --profile <name> --language <lang> --name <project> <target>` first.",
            crate::lineage::lineage_path(&layout.specdev_root).display(),
        )
    })?;

    let mut report = MigrateReport {
        dry_run,
        ..MigrateReport::default()
    };

    // ----- Profile shell -----
    plan_profile_shell(plugin_root, &meta.profile, layout, dry_run, &mut report)?;

    // ----- Blocks -----
    let block_names: Vec<String> = lineage.blocks.keys().cloned().collect();
    for name in &block_names {
        match collect_block_files(plugin_root, name) {
            Ok(files) => {
                let new_hash = rollup_hash(&files);
                let entry = lineage
                    .blocks
                    .get(name)
                    .expect("block key just iterated must still exist");
                if entry.content_hash == new_hash {
                    report.skipped.push(SkippedUnit {
                        kind: UnitKind::Block,
                        name: name.clone(),
                        hash: new_hash,
                    });
                    continue;
                }
                let written = apply_unit_migration(
                    layout,
                    UnitKind::Block,
                    name,
                    &files,
                    dry_run,
                )?;
                report.migrated.push(MigratedUnit {
                    kind: UnitKind::Block,
                    name: name.clone(),
                    before_hash: entry.content_hash.clone(),
                    after_hash: new_hash.clone(),
                    files_written: written,
                });
                if !dry_run {
                    if let Some(e) = lineage.blocks.get_mut(name) {
                        e.content_hash = new_hash;
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("[INLINE-001]") {
                    report
                        .missing_in_plugin
                        .push(format!("[MIGRATE-002] block `{name}` not found in plugin tree at {}", plugin_root.join("blocks").join(name).display()));
                } else {
                    return Err(e);
                }
            }
        }
    }

    // ----- Bridges -----
    let bridge_names: Vec<String> = lineage.bridges.keys().cloned().collect();
    for name in &bridge_names {
        match collect_bridge_files(plugin_root, name) {
            Ok(files) => {
                let new_hash = rollup_hash(&files);
                let entry = lineage
                    .bridges
                    .get(name)
                    .expect("bridge key just iterated must still exist");
                if entry.content_hash == new_hash {
                    report.skipped.push(SkippedUnit {
                        kind: UnitKind::Bridge,
                        name: name.clone(),
                        hash: new_hash,
                    });
                    continue;
                }
                let written = apply_unit_migration(
                    layout,
                    UnitKind::Bridge,
                    name,
                    &files,
                    dry_run,
                )?;
                report.migrated.push(MigratedUnit {
                    kind: UnitKind::Bridge,
                    name: name.clone(),
                    before_hash: entry.content_hash.clone(),
                    after_hash: new_hash.clone(),
                    files_written: written,
                });
                if !dry_run {
                    if let Some(e) = lineage.bridges.get_mut(name) {
                        e.content_hash = new_hash;
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("[INLINE-001]") {
                    report
                        .missing_in_plugin
                        .push(format!("[MIGRATE-002] bridge `{name}` not found in plugin tree at {}", plugin_root.join("bridges").join(name).display()));
                } else {
                    return Err(e);
                }
            }
        }
    }

    // Persist lineage iff anything actually migrated. Skip the write when
    // dry-run OR when nothing changed (preserves byte-stability of the
    // file's mtime + content for idempotency tests).
    if !dry_run && !report.migrated.is_empty() {
        lineage.save(&layout.specdev_root)?;
    }

    Ok(report)
}

/// Plan + apply the profile-shell migration: copy `profile.yaml` and any
/// `templates/*` from `<plugin-root>/profiles/<name>/`. Profile shell has
/// no per-unit `content_hash` (lineage `[meta]` is the audit trail and is
/// frozen); migration is purely "write missing files; never overwrite."
fn plan_profile_shell(
    plugin_root: &Path,
    profile_name: &str,
    layout: &LayoutContext,
    dry_run: bool,
    report: &mut MigrateReport,
) -> Result<()> {
    let profile_dir = plugin_root.join("profiles").join(profile_name);
    if !profile_dir.is_dir() {
        // Profile not in plugin tree — surface as MIGRATE-002 alongside
        // missing blocks/bridges. The lineage `[meta]` already named this
        // profile at bootstrap time; we can't recover the manifest from
        // anywhere else.
        report.missing_in_plugin.push(format!(
            "[MIGRATE-002] profile `{profile_name}` not found in plugin tree at {}",
            profile_dir.display(),
        ));
        return Ok(());
    }

    // 1. profile.yaml — write only when source exists AND destination missing.
    let manifest_src = profile_dir.join("profile.yaml");
    if manifest_src.is_file() {
        let dst_rel = PathBuf::from("profile.yaml");
        let dst_abs = layout.specdev_root.join(&dst_rel);
        if !dst_abs.exists() {
            if !dry_run {
                if let Some(parent) = dst_abs.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        anyhow!("creating {}: {e}", parent.display())
                    })?;
                }
                let bytes = std::fs::read(&manifest_src)
                    .map_err(|e| anyhow!("reading {}: {e}", manifest_src.display()))?;
                std::fs::write(&dst_abs, &bytes)
                    .map_err(|e| anyhow!("writing {}: {e}", dst_abs.display()))?;
            }
            report.profile_files_written.push(dst_rel);
        }
    }

    // 2. templates/* — byte-faithful copy of any flat `templates/<file>`
    //    entries the profile ships (mirrors `bootstrap::profile_plan` step
    //    3c). Subdirectories not walked in v1; if a template is missing
    //    on the destination side, copy it; pre-existing files are never
    //    overwritten.
    let templates_dir = profile_dir.join("templates");
    if templates_dir.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&templates_dir)
            .map_err(|e| anyhow!("reading {}: {e}", templates_dir.display()))?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        entries.sort();
        for src in entries {
            let Some(file_name) = src.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let dst_rel = PathBuf::from(format!("templates/{file_name}"));
            let dst_abs = layout.specdev_root.join(&dst_rel);
            if dst_abs.exists() {
                continue;
            }
            if !dry_run {
                if let Some(parent) = dst_abs.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        anyhow!("creating {}: {e}", parent.display())
                    })?;
                }
                let bytes = std::fs::read(&src)
                    .map_err(|e| anyhow!("reading {}: {e}", src.display()))?;
                std::fs::write(&dst_abs, &bytes)
                    .map_err(|e| anyhow!("writing {}: {e}", dst_abs.display()))?;
            }
            report.profile_files_written.push(dst_rel);
        }
    }

    if report.profile_files_written.is_empty() {
        report.profile_skipped = true;
    }
    Ok(())
}

/// Apply (or plan, when `dry_run == true`) one block/bridge unit's
/// migration. Returns the list of relative destination paths that were
/// written (or would be). Files already present on disk are NEVER
/// overwritten — migrate is purely additive.
fn apply_unit_migration(
    layout: &LayoutContext,
    kind: UnitKind,
    name: &str,
    files: &[UnitFile],
    dry_run: bool,
) -> Result<Vec<PathBuf>> {
    let specdev_root = &layout.specdev_root;
    // Filter to only files whose destinations are missing — never
    // overwrite existing project copies (additive migrate).
    let to_write: Vec<UnitFile> = files
        .iter()
        .filter(|f| !specdev_root.join(&f.rel_dst).exists())
        .cloned()
        .collect();

    let written_paths: Vec<PathBuf> = to_write.iter().map(|f| f.rel_dst.clone()).collect();

    if dry_run {
        return Ok(written_paths);
    }

    if !to_write.is_empty() {
        write_unit_files(specdev_root, &to_write)?;
    }

    // Rewrite the upstream snapshot with the FULL plugin-HEAD file set so
    // the recorded merge-base matches the new content_hash. The previous
    // upstream snapshot reflected the pre-D52 (no-manifest) bytes; after
    // migrate it must reflect what the lineage hash now witnesses.
    let upstream_files: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|f| (f.rel_src_in_unit.clone(), f.bytes.clone()))
        .collect();
    upstream::write_upstream_files(specdev_root, kind, name, &upstream_files)?;

    Ok(written_paths)
}
