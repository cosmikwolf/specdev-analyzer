//! `specdev-analyzer migrate` — populate manifests for pre-manifest projects.
//!
//! The backwards-compat path: projects bootstrapped before manifests existed
//! ship no `profile.yaml` / `deps.yaml` / `contract.yaml` in
//! their inlined tree. `migrate` walks the project's lineage, resolves which
//! profile / blocks / bridges it has, and byte-copies the missing manifests
//! from the plugin tree's HEAD. Lineage `content_hash` is updated to the
//! new (post-manifest) rolled-up hash; `origin` / `origin_version` /
//! `inlined_at` stay frozen — they record when the unit was *originally*
//! inlined, not when it was migrated.
//!
//! Idempotency: when every lineage entry's hash already matches the plugin-
//! HEAD rolled-up hash, migrate is a true no-op (zero file writes). When
//! the plugin tree itself is pre-manifest (no manifests), every collector
//! returns the legacy file set and the rolled-up hash matches lineage —
//! migrate becomes a no-op for those units. This is correct: there is
//! nothing to migrate to.
//!
//! Project-local bridges (`reductions/bridges/local/...`) have no lineage
//! entry and are skipped — there is no plugin source to byte-copy
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

use crate::bootstrap::{self, BootstrapInputs};
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
    /// inlined profile per project).
    pub profile_files_written: Vec<PathBuf>,
    /// `--refresh` only: files overwritten because their on-disk hash
    /// still matched the hash `bootstrap` recorded, which proves nobody
    /// hand-edited them (#109).
    pub profile_files_refreshed: Vec<PathBuf>,
    /// `--refresh` only: files left alone because they diverge from the
    /// recorded hash. Refusing here is the point — an overwrite would
    /// discard the user's edit with no merge base to recover it from.
    pub profile_files_locally_edited: Vec<PathBuf>,
    /// `--refresh` only: co-owned aggregators whose profile-owned part the
    /// plugin has moved. Detectable, but not mechanically applicable —
    /// rewriting the file would drop the `$ref` list `inline` appended.
    pub profile_files_needs_manual: Vec<PathBuf>,
    /// `--refresh` on a tree with no `[profile_files]`. Nothing can be
    /// proven untouched, so nothing is written.
    pub profile_refresh_unavailable: bool,
    pub profile_skipped: bool,
    pub migrated: Vec<MigratedUnit>,
    pub skipped: Vec<SkippedUnit>,
    /// Units whose plugin copy moved while no file is missing: left as they
    /// are, so `drift` keeps reporting the move.
    pub plugin_moved: Vec<SkippedUnit>,
    pub missing_in_plugin: Vec<String>,
    pub dry_run: bool,
}

impl MigrateReport {
    /// Total file-write count across every per-unit migration plus the
    /// profile shell.
    pub fn total_files_written(&self) -> usize {
        self.profile_files_written.len()
            + self.profile_files_refreshed.len()
            + self
                .migrated
                .iter()
                .map(|m| m.files_written.len())
                .sum::<usize>()
    }
}

/// Run `specdev-analyzer migrate`. The CLI calls this; the function
/// performs detection, planning, and (when `dry_run == false`) the file
/// writes + lineage update.
pub fn run_migrate(
    layout: &LayoutContext,
    plugin_root: &Path,
    dry_run: bool,
    refresh: bool,
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
    let mut new_profile_hashes: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    plan_profile_shell(
        plugin_root,
        &meta.profile,
        layout,
        dry_run,
        refresh,
        &lineage.profile_files.clone(),
        &mut new_profile_hashes,
        &mut report,
    )?;

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
                // Nothing missing: the plugin moved content the project already
                // has. Re-recording would hide that from `drift`; leave it.
                if !files.iter().any(|f| !layout.specdev_root.join(&f.rel_dst).exists()) {
                    report.plugin_moved.push(SkippedUnit { kind: UnitKind::Block, name: name.clone(), hash: new_hash });
                    continue;
                }
                let written = apply_unit_migration(layout, UnitKind::Block, name, &files, dry_run)?;
                report.migrated.push(MigratedUnit {
                    kind: UnitKind::Block,
                    name: name.clone(),
                    before_hash: entry.content_hash.clone(),
                    after_hash: new_hash.clone(),
                    files_written: written,
                });
                if !dry_run {
                    let snap = upstream::read_upstream_unit(&layout.specdev_root, UnitKind::Block, name)?;
                    if let Some(e) = lineage.blocks.get_mut(name) {
                        e.content_hash = crate::inline::snapshot_rollup(UnitKind::Block, name, &snap);
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("[INLINE-001]") {
                    report.missing_in_plugin.push(format!(
                        "[MIGRATE-002] block `{name}` not found in plugin tree at {}",
                        plugin_root.join("blocks").join(name).display()
                    ));
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
                if !files.iter().any(|f| !layout.specdev_root.join(&f.rel_dst).exists()) {
                    report.plugin_moved.push(SkippedUnit { kind: UnitKind::Bridge, name: name.clone(), hash: new_hash });
                    continue;
                }
                let written =
                    apply_unit_migration(layout, UnitKind::Bridge, name, &files, dry_run)?;
                report.migrated.push(MigratedUnit {
                    kind: UnitKind::Bridge,
                    name: name.clone(),
                    before_hash: entry.content_hash.clone(),
                    after_hash: new_hash.clone(),
                    files_written: written,
                });
                if !dry_run {
                    let snap = upstream::read_upstream_unit(&layout.specdev_root, UnitKind::Bridge, name)?;
                    if let Some(e) = lineage.bridges.get_mut(name) {
                        e.content_hash = crate::inline::snapshot_rollup(UnitKind::Bridge, name, &snap);
                    }
                }
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg.contains("[INLINE-001]") {
                    report.missing_in_plugin.push(format!(
                        "[MIGRATE-002] bridge `{name}` not found in plugin tree at {}",
                        plugin_root.join("bridges").join(name).display()
                    ));
                } else {
                    return Err(e);
                }
            }
        }
    }

    // Persist lineage iff anything actually migrated. Skip the write when
    // dry-run OR when nothing changed (preserves byte-stability of the
    // file's mtime + content for idempotency tests).
    // Fold the profile-file hashes in before deciding whether to write:
    // a refresh with no block/bridge migration still moves the base.
    let profile_hashes_changed = !new_profile_hashes.is_empty()
        && new_profile_hashes
            .iter()
            .any(|(k, v)| lineage.profile_files.get(k) != Some(v));
    if profile_hashes_changed {
        lineage.profile_files.extend(new_profile_hashes);
    }
    if !dry_run && (!report.migrated.is_empty() || profile_hashes_changed) {
        lineage.save(&layout.specdev_root)?;
    }

    Ok(report)
}

/// Plan + apply the profile-shell migration.
///
/// Two modes.
///
/// **Default (additive).** Write only files that are MISSING from the
/// project; never overwrite. This is the pre-#109 contract and it is
/// unchanged.
///
/// **`--refresh`.** Also rewrite a file whose on-disk hash still equals
/// the hash `bootstrap` recorded in `[profile_files]`. Equality is a proof
/// that nobody hand-edited it, so the overwrite cannot destroy work. A
/// file that diverges is reported and left alone: there is no `upstream/`
/// base copy for profile files, so there is nothing to merge against and
/// an overwrite would simply lose the edit. `drift` names those files;
/// resolving them is the user's call, not a flag's.
///
/// Refresh needs the recorded hashes. A tree bootstrapped before #109 has
/// none, so nothing there can be proven untouched, and the mode reports
/// `profile_refresh_unavailable` rather than guessing.
///
/// The file list comes from [`bootstrap::profile_expected_bytes`] — the
/// same producer `bootstrap` and `drift` use. The earlier hand-rolled
/// version enumerated `profile.yaml` and `templates/*` only, so it could
/// not repair the stale `architecture.schema.yaml` that #109 measured;
/// `schemas/`, `codegen/` and the root TOMLs were invisible to it.
#[allow(clippy::too_many_arguments)]
fn plan_profile_shell(
    plugin_root: &Path,
    profile_name: &str,
    layout: &LayoutContext,
    dry_run: bool,
    refresh: bool,
    recorded: &std::collections::BTreeMap<String, String>,
    // Hashes to fold back into `[profile_files]` for every file this call
    // wrote. A write that does not move the recorded base leaves `drift`
    // reporting `Both` on a file it just made clean — the base is now
    // wrong on both sides of the comparison. Hash after the step that
    // writes.
    new_hashes: &mut std::collections::BTreeMap<String, String>,
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

    if refresh && recorded.is_empty() {
        report.profile_refresh_unavailable = true;
    }

    let inputs = BootstrapInputs {
        profile: profile_name.to_string(),
        language: layout.spec.project.language.clone(),
        project_name: layout.spec.project.name.clone(),
        target_dir: layout.project_root.clone(),
    };
    let expected = bootstrap::profile_expected_bytes(&profile_dir, &inputs)?;

    for (rel, bytes) in &expected {
        let dst_rel = PathBuf::from(rel);
        let dst_abs = layout.project_root.join(&dst_rel);

        if !dst_abs.exists() {
            // Recorded but missing: deleted on purpose; `drift` reports it.
            if recorded.contains_key(rel) {
                continue;
            }
            write_file(&dst_abs, bytes, dry_run)?;
            new_hashes.insert(rel.clone(), bootstrap::profile_file_hash(rel, bytes)?);
            report.profile_files_written.push(dst_rel);
            continue;
        }

        if !refresh {
            continue;
        }

        // A co-owned aggregator is never rewritten. `module.schema.yaml`
        // holds the `$ref` list `inline` appended, and the plugin's
        // template does not; writing the template back would silently
        // delete every inlined unit's schema reference. Its recorded hash
        // covers only the profile-owned remainder, so a real upstream move
        // is detectable but not mechanically applicable — say that instead
        // of either destroying the refs or reporting nothing.
        if bootstrap::CO_OWNED_AGGREGATORS.contains(&rel.as_str()) {
            let current = std::fs::read(&dst_abs)
                .map_err(|e| anyhow!("reading {}: {e}", dst_abs.display()))?;
            let ours = bootstrap::profile_file_hash(rel, &current)?;
            let theirs = bootstrap::profile_file_hash(rel, bytes)?;
            if ours != theirs {
                report.profile_files_needs_manual.push(dst_rel);
            }
            continue;
        }

        let Some(base) = recorded.get(rel) else {
            // Present on disk, never recorded. Refresh has no proof of
            // provenance, so it declines — same reasoning as a diverged
            // file, reported under the same list.
            report.profile_files_locally_edited.push(dst_rel);
            continue;
        };
        let current =
            std::fs::read(&dst_abs).map_err(|e| anyhow!("reading {}: {e}", dst_abs.display()))?;
        // `profile_file_hash` is the function that produced `base`, so it
        // is the one that may compare against it. Today the co-owned
        // branch above catches every path whose recorded hash is
        // normalized, so this call is equivalent to `content_hash` for all
        // reachable input — it is the invariant written down, not a live
        // guard, and no test can distinguish the two. The version that
        // hard-coded `content_hash` here did produce the bug (a freshly
        // bootstrapped tree reporting `module.schema.yaml` as user-edited),
        // before the co-owned branch existed to mask it.
        if !bootstrap::profile_file_matches(rel, &current, base)? {
            report.profile_files_locally_edited.push(dst_rel);
            continue;
        }
        // Provably untouched since bootstrap. Skip a no-op rewrite so the
        // report counts real changes only.
        if current.as_slice() == bytes.as_slice() {
            continue;
        }
        write_file(&dst_abs, bytes, dry_run)?;
        new_hashes.insert(rel.clone(), bootstrap::profile_file_hash(rel, bytes)?);
        report.profile_files_refreshed.push(dst_rel);
    }

    if report.profile_files_written.is_empty() && report.profile_files_refreshed.is_empty() {
        report.profile_skipped = true;
    }
    Ok(())
}

/// Write `bytes` to `path`, creating parents. No-op under `dry_run`.
fn write_file(path: &Path, bytes: &[u8], dry_run: bool) -> Result<()> {
    if dry_run {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow!("creating {}: {e}", parent.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| anyhow!("writing {}: {e}", path.display()))?;
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

    // Add only the files just written to the upstream snapshot. The other
    // snapshot files keep their bytes: they are the merge base of files the
    // project already has, and replacing them with the plugin's current copy
    // would hide a plugin move from `drift` and corrupt `--merge`.
    let upstream_files: Vec<(PathBuf, Vec<u8>)> = to_write
        .iter()
        .map(|f| (f.rel_src_in_unit.clone(), f.bytes.clone()))
        .collect();
    upstream::write_upstream_files(specdev_root, kind, name, &upstream_files)?;

    Ok(written_paths)
}
