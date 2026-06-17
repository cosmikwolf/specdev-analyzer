//! `specdev inline <block> --merge` and `specdev inline-bridge <a>+<b> --merge`.
//!
//! Line-based 3-way merge via `git merge-file`.
//! For each file in the unit, three inputs:
//!
//! - `base`   — bytes-as-of-inlining, read from `specs/specdev/upstream/...`
//! - `ours`   — current project copy, read from `specs/specdev/{schemas,reductions,codegen}/...`
//! - `theirs` — current plugin bytes, re-fetched via [`crate::inline::collect_block_files`]
//!
//! On clean (`git merge-file` exit 0): replace `ours`, advance `upstream`,
//! update lineage. On conflict (exit > 0): write merged-with-markers bytes
//! to `ours`, leave `upstream` and lineage unchanged so subsequent retries
//! merge from the same base.
//!
//! Strict v1: the file-set must match across (upstream, current plugin).
//! Any mismatch errors `MERGE-FILESET` rather than guessing intent —
//! file-set evolution (added/removed files in a unit) is a future widening.
//!
//! No `--force`: git owns destructive resets.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::hash::content_hash;
use crate::inline::{collect_block_files, collect_bridge_files, lex_order_bridge, UnitFile};
use crate::layout::LayoutContext;
use crate::lineage::{Entry, Lineage};
use crate::upstream::{self, UnitKind};

/// Result of a merge operation. Used by the CLI handler for output and
/// by tests for assertions.
#[derive(Debug, Clone)]
pub struct MergeOutcome {
    pub unit_kind: UnitKind,
    pub unit_name: String,
    /// `true` iff every file merged cleanly (no conflict markers, no
    /// fileset mismatches). When `clean`, the lineage and upstream were
    /// advanced; otherwise everything is left in place for retry.
    pub clean: bool,
    /// Plugin version the merge advanced the lineage to. `Some` only when
    /// `clean == true`.
    pub advanced_to_version: Option<String>,
    /// Files that contained conflict markers after `git merge-file`.
    pub conflicted_files: Vec<PathBuf>,
}

/// Read the plugin's `plugin.json` `version` field. Mirrors
/// [`crate::inline`]'s private helper; lifted to merge so we don't have to
/// promote the inline private. Both still hit the same marker file.
fn plugin_version(plugin_root: &Path) -> Result<String> {
    let manifest = plugin_root.join(crate::plugin_root::PLUGIN_MARKER);
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .with_context(|| format!("parsing {}", manifest.display()))?;
    Ok(value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string())
}

fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Run a 3-way merge for one block.
pub fn run_merge_block(
    layout: &LayoutContext,
    plugin_root: &Path,
    name: &str,
) -> Result<MergeOutcome> {
    run_merge_unit(layout, plugin_root, UnitKind::Block, name)
}

/// Run a 3-way merge for one bridge.
pub fn run_merge_bridge(
    layout: &LayoutContext,
    plugin_root: &Path,
    name: &str,
) -> Result<MergeOutcome> {
    // Same lex-order discipline that runtime inline-bridge applies.
    let _ = lex_order_bridge(name)?;
    run_merge_unit(layout, plugin_root, UnitKind::Bridge, name)
}

fn run_merge_unit(
    layout: &LayoutContext,
    plugin_root: &Path,
    kind: UnitKind,
    name: &str,
) -> Result<MergeOutcome> {
    // Preflight: lineage must record the unit. `--merge` against a never-
    // inlined unit is a category error; user wants plain `inline`.
    let mut lineage = Lineage::load(&layout.specdev_root)?;
    let already = match kind {
        UnitKind::Block => lineage.has_block(name),
        UnitKind::Bridge => lineage.has_bridge(name),
    };
    if !already {
        let kind_word = match kind {
            UnitKind::Block => "block",
            UnitKind::Bridge => "bridge",
        };
        let cmd_word = match kind {
            UnitKind::Block => "inline",
            UnitKind::Bridge => "inline-bridge",
        };
        return Err(anyhow!(
            "[MERGE-003] {kind_word} `{name}` has no lineage entry; use `specdev {cmd_word} {name}` (without --merge) for an initial inline",
        ));
    }

    // Collect upstream bytes (the merge base). An empty upstream is a
    // broken-invariant case — drift handles diagnostics; merge bails.
    let upstream_files = upstream::read_upstream_unit(&layout.specdev_root, kind, name)?;
    if upstream_files.is_empty() {
        return Err(anyhow!(
            "[MERGE-FILESET] {kind:?} `{name}`: upstream/ snapshot is empty; cannot 3-way merge. Run `specdev drift` to inspect; the project tree may be corrupted.",
        ));
    }

    // Collect plugin bytes (theirs).
    let plugin_files = match kind {
        UnitKind::Block => collect_block_files(plugin_root, name)?,
        UnitKind::Bridge => collect_bridge_files(plugin_root, name)?,
    };

    // File-set match check — strict v1.
    let upstream_keys: std::collections::BTreeSet<&Path> =
        upstream_files.keys().map(|p| p.as_path()).collect();
    let plugin_keys: std::collections::BTreeSet<&Path> =
        plugin_files.iter().map(|f| f.rel_src_in_unit.as_path()).collect();
    if upstream_keys != plugin_keys {
        let only_upstream: Vec<&Path> =
            upstream_keys.difference(&plugin_keys).copied().collect();
        let only_plugin: Vec<&Path> = plugin_keys.difference(&upstream_keys).copied().collect();
        return Err(anyhow!(
            "[MERGE-FILESET] {kind:?} `{name}`: file set changed since inlining (only in upstream: {only_upstream:?}; only in plugin: {only_plugin:?}). File-set evolution is not supported in v1; intervene manually and re-`inline`.",
        ));
    }

    // Per-file merge. Collect outcomes; commit only if all clean.
    let mut merged: Vec<(UnitFile, Vec<u8>, bool)> = Vec::new();
    let mut conflicted: Vec<PathBuf> = Vec::new();
    let tmp_dir = std::env::temp_dir().join(format!(
        "specdev-merge-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(&tmp_dir)
        .with_context(|| format!("creating {}", tmp_dir.display()))?;

    for f in &plugin_files {
        let base = upstream_files
            .get(&f.rel_src_in_unit)
            .ok_or_else(|| {
                anyhow!(
                    "[MERGE-FILESET] {kind:?} `{name}`: upstream missing {} (file-set check should have caught this)",
                    f.rel_src_in_unit.display()
                )
            })?
            .clone();
        let ours_path = layout.specdev_root.join(&f.rel_dst);
        let ours = std::fs::read(&ours_path)
            .with_context(|| format!("reading {}", ours_path.display()))?;

        let (merged_bytes, clean) = merge_three(&tmp_dir, &ours, &base, &f.bytes, &f.rel_dst)?;
        if !clean {
            conflicted.push(f.rel_dst.clone());
        }
        merged.push((f.clone(), merged_bytes, clean));
    }

    let _ = std::fs::remove_dir_all(&tmp_dir);

    let all_clean = conflicted.is_empty();
    if !all_clean {
        // Write conflict-marker bytes to ours; leave upstream + lineage
        // unchanged so retry from the same base is possible.
        for (f, bytes, _clean) in &merged {
            let ours_path = layout.specdev_root.join(&f.rel_dst);
            std::fs::write(&ours_path, bytes)
                .with_context(|| format!("writing {}", ours_path.display()))?;
        }
        return Err(anyhow!(
            "[MERGE-002] {kind:?} `{name}`: merge produced conflicts in {} file(s); resolve markers in {:?} and commit; rerun `specdev inline{} {name} --merge` to retry from the same base.",
            conflicted.len(),
            conflicted,
            if matches!(kind, UnitKind::Bridge) {
                "-bridge"
            } else {
                ""
            },
        ));
    }

    // Clean merge: write merged ours, advance upstream, advance lineage.
    for (f, bytes, _) in &merged {
        let ours_path = layout.specdev_root.join(&f.rel_dst);
        std::fs::write(&ours_path, bytes)
            .with_context(|| format!("writing {}", ours_path.display()))?;
    }
    let upstream_new: Vec<(PathBuf, Vec<u8>)> = plugin_files
        .iter()
        .map(|f| (f.rel_src_in_unit.clone(), f.bytes.clone()))
        .collect();
    upstream::write_upstream_files(&layout.specdev_root, kind, name, &upstream_new)?;

    // Recompute the rolled-up content_hash over the *merged* per-unit
    // file set (the bytes the project will see going forward). Since the
    // merge was clean the merged bytes equal the plugin bytes for files
    // where local was untouched, equal local for files where plugin was
    // untouched, and a 3-way blend for both-edited-but-non-conflicting
    // files. The hash witnesses what's deployed.
    let new_hash = rollup_merged(&merged);
    let new_version = plugin_version(plugin_root)?;
    let entry = Entry {
        origin: "plugin".into(),
        origin_version: new_version.clone(),
        inlined_at: now_iso8601(),
        content_hash: new_hash,
    };
    match kind {
        UnitKind::Block => {
            lineage.blocks.insert(name.to_string(), entry);
        }
        UnitKind::Bridge => {
            lineage.bridges.insert(name.to_string(), entry);
        }
    }
    lineage.save(&layout.specdev_root)?;

    Ok(MergeOutcome {
        unit_kind: kind,
        unit_name: name.to_string(),
        clean: true,
        advanced_to_version: Some(new_version),
        conflicted_files: Vec::new(),
    })
}

/// Roll up the merged file set's content_hash. Mirrors
/// [`crate::inline::rollup_hash`] but takes the merged bytes (which may
/// differ from theirs/ours for compatible 3-way files). Sort is by
/// `rel_dst` to match the inline rollup.
fn rollup_merged(merged: &[(UnitFile, Vec<u8>, bool)]) -> String {
    let mut entries: Vec<(&Path, &[u8])> = merged
        .iter()
        .map(|(f, bytes, _)| (f.rel_dst.as_path(), bytes.as_slice()))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    let mut summary = String::new();
    for (rel, bytes) in entries {
        let mut h = Sha256::new();
        h.update(bytes);
        let digest = h.finalize();
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(&mut summary, "{byte:02x}");
        }
        summary.push_str("  ");
        summary.push_str(&rel.to_string_lossy().replace('\\', "/"));
        summary.push('\n');
    }
    content_hash(summary.as_bytes())
}

/// Run `git merge-file --diff3 -p ours base theirs`. Returns
/// `(merged_bytes, clean)`. `clean == true` iff exit code is 0; conflict
/// markers in `merged_bytes` indicate `clean == false`.
fn merge_three(
    tmp_dir: &Path,
    ours: &[u8],
    base: &[u8],
    theirs: &[u8],
    label: &Path,
) -> Result<(Vec<u8>, bool)> {
    // Sanitize the label into a filename-safe stem so temp paths don't
    // collide across files in the same unit.
    let stem = label.to_string_lossy().replace('/', "__").replace('\\', "__");
    let ours_path = tmp_dir.join(format!("{stem}.ours"));
    let base_path = tmp_dir.join(format!("{stem}.base"));
    let theirs_path = tmp_dir.join(format!("{stem}.theirs"));
    std::fs::write(&ours_path, ours)?;
    std::fs::write(&base_path, base)?;
    std::fs::write(&theirs_path, theirs)?;

    let output = Command::new("git")
        .arg("merge-file")
        .arg("--diff3")
        .arg("-p")
        .arg("-L")
        .arg("ours")
        .arg("-L")
        .arg("base")
        .arg("-L")
        .arg("theirs")
        .arg(&ours_path)
        .arg(&base_path)
        .arg(&theirs_path)
        .output()
        .map_err(|e| {
            anyhow!(
                "[MERGE-001] failed to invoke `git merge-file`: {e}. Install git or set PATH to include it.",
            )
        })?;

    let merged = output.stdout;
    let clean = output.status.success();
    let _ = std::fs::remove_file(&ours_path);
    let _ = std::fs::remove_file(&base_path);
    let _ = std::fs::remove_file(&theirs_path);

    // git merge-file returns >= 0 conflict count (or -1 on failure). A
    // negative status code means git failed in a way that's not "merge
    // produced conflicts" — surface it.
    if let Some(code) = output.status.code() {
        if code < 0 {
            return Err(anyhow!(
                "[MERGE-001] `git merge-file` failed with status {code}: {}",
                String::from_utf8_lossy(&output.stderr),
            ));
        }
    }

    Ok((merged, clean))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("specdev-merge-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn three_way_clean_when_only_theirs_differs() {
        let dir = tmp("clean-theirs");
        let base = b"line1\nline2\nline3\n";
        let ours = b"line1\nline2\nline3\n"; // unchanged
        let theirs = b"line1\nline2-changed\nline3\n";
        let (merged, clean) = merge_three(
            &dir,
            ours,
            base,
            theirs,
            std::path::Path::new("test.yaml"),
        )
        .unwrap();
        assert!(clean);
        assert_eq!(merged, theirs.to_vec());
    }

    #[test]
    fn three_way_clean_when_compatible_edits() {
        let dir = tmp("clean-compat");
        let base = b"line1\nline2\nline3\nline4\nline5\n";
        let ours = b"line1\nline2-ours\nline3\nline4\nline5\n";
        let theirs = b"line1\nline2\nline3\nline4-theirs\nline5\n";
        let (merged, clean) = merge_three(
            &dir,
            ours,
            base,
            theirs,
            std::path::Path::new("test.yaml"),
        )
        .unwrap();
        assert!(clean, "merge output:\n{}", String::from_utf8_lossy(&merged));
        let s = String::from_utf8(merged).unwrap();
        assert!(s.contains("line2-ours"));
        assert!(s.contains("line4-theirs"));
    }

    #[test]
    fn three_way_conflict_when_same_line_edited() {
        let dir = tmp("conflict");
        let base = b"line1\nline2\nline3\n";
        let ours = b"line1\nline2-ours\nline3\n";
        let theirs = b"line1\nline2-theirs\nline3\n";
        let (merged, clean) = merge_three(
            &dir,
            ours,
            base,
            theirs,
            std::path::Path::new("test.yaml"),
        )
        .unwrap();
        assert!(!clean);
        let s = String::from_utf8(merged).unwrap();
        assert!(s.contains("<<<<<<<"));
        assert!(s.contains("======="));
        assert!(s.contains(">>>>>>>"));
    }
}
