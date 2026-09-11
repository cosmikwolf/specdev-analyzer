//! `specs/specdev/upstream/` — bytes-as-of-inlining for every inlined unit.
//!
//! This directory is the **merge base** for `--merge` and the reference point
//! for `drift`'s 3-way comparison. It mirrors the unit's plugin-source layout:
//! `upstream/blocks/embassy/{schema.yaml, reduction.yaml, codegen/<lang>/README.md}`.
//!
//! Authoritative, not a cache: the bytes are not regenerable. The plugin
//! only carries the current version; if the plugin has moved past the
//! version recorded in lineage, the historical bytes only exist here.
//!
//! Visible (no dot-prefix), committed to git, CLI-managed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Block or bridge — typed dispatch for upstream/inline/merge/drift to
/// avoid stringly-typed kind handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    Block,
    Bridge,
}

impl UnitKind {
    fn dir_segment(&self) -> &'static str {
        match self {
            UnitKind::Block => "blocks",
            UnitKind::Bridge => "bridges",
        }
    }
}

/// `<specdev_root>/upstream/<blocks|bridges>/<name>/`.
pub fn upstream_dir(specdev_root: &Path, kind: UnitKind, name: &str) -> PathBuf {
    specdev_root
        .join("upstream")
        .join(kind.dir_segment())
        .join(name)
}

/// `true` if the upstream tree for `(kind, name)` exists as a directory.
pub fn upstream_unit_present(specdev_root: &Path, kind: UnitKind, name: &str) -> bool {
    upstream_dir(specdev_root, kind, name).is_dir()
}

/// Write a unit's upstream bytes from a list of `(rel_src_in_unit, bytes)`
/// pairs. `rel_src_in_unit` is the path relative to the unit dir in the
/// plugin (e.g. `schema.yaml`, `codegen/rust/README.md`) — *not* the
/// project-destination path. Files are written verbatim.
pub fn write_upstream_files(
    specdev_root: &Path,
    kind: UnitKind,
    name: &str,
    files: &[(PathBuf, Vec<u8>)],
) -> Result<()> {
    let unit_dir = upstream_dir(specdev_root, kind, name);
    for (rel, bytes) in files {
        let dst = unit_dir.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&dst, bytes).with_context(|| format!("writing {}", dst.display()))?;
    }
    Ok(())
}

/// Read every file under the unit's upstream tree, keyed by path relative
/// to the unit dir. Used by `merge` (base bytes) and `drift` (comparison).
pub fn read_upstream_unit(
    specdev_root: &Path,
    kind: UnitKind,
    name: &str,
) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let unit_dir = upstream_dir(specdev_root, kind, name);
    let mut out = BTreeMap::new();
    if !unit_dir.is_dir() {
        return Ok(out);
    }
    walk_unit(&unit_dir, &unit_dir, &mut out)?;
    Ok(out)
}

fn walk_unit(base: &Path, cur: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) -> Result<()> {
    for entry in std::fs::read_dir(cur).with_context(|| format!("reading {}", cur.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_unit(base, &path, out)?;
        } else if path.is_file() {
            let rel = path
                .strip_prefix(base)
                .with_context(|| format!("relativizing {}", path.display()))?
                .to_path_buf();
            let bytes =
                std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
            out.insert(rel, bytes);
        }
    }
    Ok(())
}

/// Remove the upstream tree for a unit. Used in tests and in future
/// `uninstall` flows. Idempotent: removing a missing tree is not an error.
pub fn remove_upstream_unit(specdev_root: &Path, kind: UnitKind, name: &str) -> Result<()> {
    let unit_dir = upstream_dir(specdev_root, kind, name);
    if unit_dir.is_dir() {
        std::fs::remove_dir_all(&unit_dir)
            .with_context(|| format!("removing {}", unit_dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("specdev-upstream-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn upstream_dir_layout() {
        let root = PathBuf::from("/x/specs/specdev");
        assert_eq!(
            upstream_dir(&root, UnitKind::Block, "embassy"),
            PathBuf::from("/x/specs/specdev/upstream/blocks/embassy"),
        );
        assert_eq!(
            upstream_dir(&root, UnitKind::Bridge, "embassy+lvgl"),
            PathBuf::from("/x/specs/specdev/upstream/bridges/embassy+lvgl"),
        );
    }

    #[test]
    fn write_then_read_round_trip() {
        let root = tmp("roundtrip");
        let files = vec![
            (PathBuf::from("schema.yaml"), b"$schema: ...\n".to_vec()),
            (PathBuf::from("reduction.yaml"), b"entities: []\n".to_vec()),
            (
                PathBuf::from("codegen/rust/README.md"),
                b"# embassy rust\n".to_vec(),
            ),
        ];
        write_upstream_files(&root, UnitKind::Block, "embassy", &files).unwrap();
        let read = read_upstream_unit(&root, UnitKind::Block, "embassy").unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read[&PathBuf::from("schema.yaml")], b"$schema: ...\n");
        assert_eq!(
            read[&PathBuf::from("codegen/rust/README.md")],
            b"# embassy rust\n",
        );
    }

    #[test]
    fn presence_check_is_byte_faithful() {
        let root = tmp("present");
        assert!(!upstream_unit_present(&root, UnitKind::Block, "embassy"));
        write_upstream_files(
            &root,
            UnitKind::Block,
            "embassy",
            &[(PathBuf::from("schema.yaml"), b"x".to_vec())],
        )
        .unwrap();
        assert!(upstream_unit_present(&root, UnitKind::Block, "embassy"));
    }

    #[test]
    fn missing_unit_reads_as_empty_map() {
        let root = tmp("missing");
        let m = read_upstream_unit(&root, UnitKind::Block, "embassy").unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn remove_is_idempotent() {
        let root = tmp("remove");
        remove_upstream_unit(&root, UnitKind::Block, "embassy").unwrap(); // no-op
        write_upstream_files(
            &root,
            UnitKind::Block,
            "embassy",
            &[(PathBuf::from("schema.yaml"), b"x".to_vec())],
        )
        .unwrap();
        remove_upstream_unit(&root, UnitKind::Block, "embassy").unwrap();
        assert!(!upstream_unit_present(&root, UnitKind::Block, "embassy"));
    }
}
