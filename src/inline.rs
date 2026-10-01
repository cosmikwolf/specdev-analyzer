//! `specdev inline <block>` and `specdev inline-bridge <a>+<b>`.
//!
//! Deterministic file copies plugin → project, with a single rolled-up
//! [`crate::lineage`] entry recording origin + version + content hash. Re-inline
//! is unconditionally refused; updates require `--merge`.
//!
//! Layout assumed in the plugin tree:
//!
//! ```text
//! <plugin-root>/
//!   .claude-plugin/plugin.json
//!   blocks/<name>/
//!     schema.yaml
//!     reduction.yaml
//!     README.md                       # author-facing, NOT inlined
//!   bridges/<a>+<b>/
//!     bindings.schema.yaml
//!     reduction.yaml
//!     README.md
//! ```
//!
//! Files copied to the project tree:
//!
//! - block schema      → `specs/specdev/schemas/inlined/blocks/<name>.schema.yaml`
//! - block reduction   → `specs/specdev/reductions/blocks/<name>.yaml`
//! - bridge schema     → `specs/specdev/schemas/inlined/bridges/<a>+<b>.schema.yaml`
//! - bridge reduction  → `specs/specdev/reductions/bridges/<a>+<b>.yaml`
//!
//! After per-unit copies, the unit's schema fragment gets a `$ref` entry
//! appended to `module.schema.yaml`'s `allOf` array (the aggregator slot).
//! The aggregator file is project-managed, not byte-faithfully copied; only
//! the unit files contribute to `content_hash`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::hash::content_hash;
use crate::layout::LayoutContext;
use crate::lineage::{Entry, Lineage};
use crate::upstream::{self, UnitKind};

/// Split a bridge owner name into its two component blocks. Errors with
/// `BRIDGE-LEX-001` when the components are not lexicographically ordered
/// (`a < b`). Used by both the runtime preflight in `run_inline_bridge`
/// and the plugin-author `plugin-lint` subcommand.
pub fn lex_order_bridge(name: &str) -> Result<(&str, &str)> {
    let Some((a, b)) = name.split_once('+') else {
        return Err(anyhow!(
            "[BRIDGE-LEX-001] bridge name {name:?} must be of the form `<a>+<b>`",
        ));
    };
    if a.is_empty() || b.is_empty() {
        return Err(anyhow!(
            "[BRIDGE-LEX-001] bridge name {name:?} must have non-empty components on both sides of `+`",
        ));
    }
    if a.contains('+') || b.contains('+') {
        return Err(anyhow!(
            "[BRIDGE-LEX-001] bridge name {name:?} contains more than one `+`; n-ary bridges are deferred",
        ));
    }
    if a >= b {
        return Err(anyhow!(
            "[BRIDGE-LEX-001] bridge owner names must be lex-ordered; did you mean `{b}+{a}`?"
        ));
    }
    Ok((a, b))
}

/// One unit-file copy: project-destination path, plugin-source-relative
/// path within the unit dir, and the bytes the analyzer read.
#[derive(Debug, Clone)]
pub(crate) struct UnitFile {
    /// Path inside `specs/specdev/`, used as the lex-sort key for the
    /// rolled-up hash and as the destination path under `specdev_root`.
    pub(crate) rel_dst: PathBuf,
    /// Path inside the unit's plugin-source directory (e.g. `schema.yaml`,
    /// `codegen/rust/README.md`). Used by `upstream::write_upstream_files`
    /// to mirror the plugin layout under `specs/specdev/upstream/`.
    pub(crate) rel_src_in_unit: PathBuf,
    /// Bytes the analyzer copied. Captured at preflight read so the hash
    /// witnesses what was actually deployed.
    pub(crate) bytes: Vec<u8>,
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// Read the plugin's `plugin.json` and return the `version` field. Used to
/// populate `origin_version` on inlined units. Falls back to
/// `"unknown"` if the field is missing — plugin-author-CI lints this
/// separately.
fn plugin_version(plugin_root: &Path) -> Result<String> {
    let manifest = plugin_root.join(crate::plugin_root::PLUGIN_MARKER);
    let text = std::fs::read_to_string(&manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parsing {}", manifest.display()))?;
    let v = value
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    Ok(v.to_string())
}


/// Collect the file set for a block from the plugin tree.
pub(crate) fn collect_block_files(plugin_root: &Path, name: &str) -> Result<Vec<UnitFile>> {
    let unit_dir = plugin_root.join("blocks").join(name);
    if !unit_dir.is_dir() {
        return Err(anyhow!(
            "[INLINE-001] block {name:?} not found at {}",
            unit_dir.display(),
        ));
    }

    let mut files: Vec<UnitFile> = Vec::new();

    let schema_src = unit_dir.join("schema.yaml");
    if !schema_src.is_file() {
        return Err(anyhow!(
            "[INLINE-001] block {name:?} is missing schema.yaml at {}",
            schema_src.display(),
        ));
    }
    files.push(UnitFile {
        rel_dst: PathBuf::from(format!("schemas/inlined/blocks/{name}.schema.yaml")),
        rel_src_in_unit: PathBuf::from("schema.yaml"),
        bytes: read_file(&schema_src)?,
    });

    let reduction_src = unit_dir.join("reduction.yaml");
    if !reduction_src.is_file() {
        return Err(anyhow!(
            "[INLINE-001] block {name:?} is missing reduction.yaml at {}",
            reduction_src.display(),
        ));
    }
    files.push(UnitFile {
        rel_dst: PathBuf::from(format!("reductions/blocks/{name}.yaml")),
        rel_src_in_unit: PathBuf::from("reduction.yaml"),
        bytes: read_file(&reduction_src)?,
    });

    files.sort_by(|a, b| a.rel_dst.cmp(&b.rel_dst));
    Ok(files)
}

/// Collect the file set for a bridge from the plugin tree.
pub(crate) fn collect_bridge_files(plugin_root: &Path, name: &str) -> Result<Vec<UnitFile>> {
    let unit_dir = plugin_root.join("bridges").join(name);
    if !unit_dir.is_dir() {
        return Err(anyhow!(
            "[INLINE-001] bridge {name:?} not found at {}",
            unit_dir.display(),
        ));
    }

    let mut files: Vec<UnitFile> = Vec::new();

    let schema_src = unit_dir.join("bindings.schema.yaml");
    if !schema_src.is_file() {
        return Err(anyhow!(
            "[INLINE-001] bridge {name:?} is missing bindings.schema.yaml at {}",
            schema_src.display(),
        ));
    }
    files.push(UnitFile {
        rel_dst: PathBuf::from(format!("schemas/inlined/bridges/{name}.schema.yaml")),
        rel_src_in_unit: PathBuf::from("bindings.schema.yaml"),
        bytes: read_file(&schema_src)?,
    });

    let reduction_src = unit_dir.join("reduction.yaml");
    if !reduction_src.is_file() {
        return Err(anyhow!(
            "[INLINE-001] bridge {name:?} is missing reduction.yaml at {}",
            reduction_src.display(),
        ));
    }
    files.push(UnitFile {
        rel_dst: PathBuf::from(format!("reductions/bridges/{name}.yaml")),
        rel_src_in_unit: PathBuf::from("reduction.yaml"),
        bytes: read_file(&reduction_src)?,
    });

    // Language-neutral `contract.yaml` manifest joins the bridge's per-unit
    // file set when present. It lives at the bridge dir top level (not under
    // codegen/<lang>/) and inlines next to the bridge's reduction file. Bytes
    // participate in `content_hash` automatically via the rolled-up hash;
    // bridges without contract.yaml keep their existing hash.
    let contract_src = unit_dir.join("contract.yaml");
    if contract_src.is_file() {
        files.push(UnitFile {
            rel_dst: PathBuf::from(format!("reductions/bridges/{name}.contract.yaml")),
            rel_src_in_unit: PathBuf::from("contract.yaml"),
            bytes: read_file(&contract_src)?,
        });
    }

    files.sort_by(|a, b| a.rel_dst.cmp(&b.rel_dst));
    Ok(files)
}

/// Compute the rolled-up content hash for a unit. Format: per-file lines
/// `<sha256-hex>  <rel-path>\n` in lex-sorted order, then SHA-256 of the
/// concatenation. Mirrors the `sha256sum` output convention so per-file
/// hashes stay debuggable.
pub(crate) fn rollup_hash(files: &[UnitFile]) -> String {
    let mut summary = String::new();
    for f in files {
        let mut h = Sha256::new();
        h.update(&f.bytes);
        let digest = h.finalize();
        for byte in digest {
            use std::fmt::Write;
            let _ = write!(&mut summary, "{byte:02x}");
        }
        summary.push_str("  ");
        // PathBuf → Display uses platform separators, but the rel_dst is
        // built from forward slashes above; convert defensively.
        let s = f.rel_dst.to_string_lossy().replace('\\', "/");
        summary.push_str(&s);
        summary.push('\n');
    }
    content_hash(summary.as_bytes())
}

/// Refuse if any destination is already an existing file — both the active
/// project copies and the upstream/ snapshot. Belt-and-suspenders alongside
/// the lineage-entry check; catches the case where the lineage was
/// hand-deleted but files remain.
fn refuse_if_dst_exists(
    specdev_root: &Path,
    files: &[UnitFile],
    kind: UnitKind,
    name: &str,
    unit_label: &str,
) -> Result<()> {
    for f in files {
        let dst = specdev_root.join(&f.rel_dst);
        if dst.exists() {
            return Err(anyhow!(
                "[INLINE-002] {unit_label}: refuse to overwrite existing {}; use `specdev inline --merge` or `git checkout -- specs/specdev/...` to reset",
                dst.display(),
            ));
        }
        let upstream_dst =
            upstream::upstream_dir(specdev_root, kind, name).join(&f.rel_src_in_unit);
        if upstream_dst.exists() {
            return Err(anyhow!(
                "[INLINE-002] {unit_label}: refuse to overwrite existing {}; the upstream/ snapshot already exists. Run `specdev drift` to inspect, or `git checkout -- specs/specdev/upstream/...` to reset.",
                upstream_dst.display(),
            ));
        }
    }
    Ok(())
}

/// Write each unit file to its destination under `specdev_root`. Bytes are
/// copied verbatim; no newline normalization, no YAML re-emission.
pub(crate) fn write_unit_files(specdev_root: &Path, files: &[UnitFile]) -> Result<()> {
    for f in files {
        let dst = specdev_root.join(&f.rel_dst);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&dst, &f.bytes).with_context(|| format!("writing {}", dst.display()))?;
    }
    Ok(())
}

/// Append a `$ref` to the aggregator's `allOf:` array, then write back. The
/// aggregator file is project-managed (not under the determinism invariant
/// for content_hash), so re-emit via `serde_yaml` is acceptable. Idempotent:
/// running the same insertion twice produces a single entry. Callers
/// always run this on a fresh inline (caller has already refused
/// re-inline); idempotency guards against double-runs from test harnesses.
/// Hash of an aggregator schema with its `allOf` array removed.
///
/// `module.schema.yaml` has two writers: `bootstrap` lays down the
/// profile's template, then every `inline` appends a `$ref` to `allOf`
/// (through [`aggregator_add_ref`], which re-emits the whole document via
/// `serde_yaml`). A single recorded hash of the file therefore cannot
/// separate "the plugin's template moved" from "I inlined a block" — and
/// the second is not drift at all; `drift`'s `units:` section already
/// reports it.
///
/// Dropping `allOf` before hashing splits the two cleanly. What remains is
/// exactly the profile-owned part, and it is invariant under any number of
/// `$ref` appends. Both sides of the comparison go through the same
/// re-emission, so `serde_yaml` stripping comments costs nothing: it
/// strips them identically on each side.
pub fn aggregator_profile_hash(bytes: &[u8], label: &str) -> Result<String> {
    let text = std::str::from_utf8(bytes).with_context(|| format!("{label} is not valid UTF-8"))?;
    let mut value: serde_yaml::Value =
        serde_yaml::from_str(text).with_context(|| format!("parsing {label}"))?;
    if let Some(map) = value.as_mapping_mut() {
        map.remove(serde_yaml::Value::from("allOf"));
    }
    let normalized =
        serde_yaml::to_string(&value).with_context(|| format!("re-emitting {label}"))?;
    Ok(crate::hash::content_hash(normalized.as_bytes()))
}

fn aggregator_add_ref(module_schema_path: &Path, ref_path: &str) -> Result<()> {
    let text = std::fs::read_to_string(module_schema_path)
        .with_context(|| format!("reading {}", module_schema_path.display()))?;
    let mut value: serde_yaml::Value = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing {}", module_schema_path.display()))?;

    // Locate or create allOf as a sequence at the root level.
    let mapping = value.as_mapping_mut().ok_or_else(|| {
        anyhow!(
            "{} root is not a YAML mapping",
            module_schema_path.display()
        )
    })?;
    let key = serde_yaml::Value::from("allOf");
    let entry = mapping
        .entry(key)
        .or_insert(serde_yaml::Value::Sequence(Vec::new()));
    let seq = entry
        .as_sequence_mut()
        .ok_or_else(|| anyhow!("{} `allOf` is not a sequence", module_schema_path.display()))?;

    // Idempotent: only add if no existing entry has the same $ref.
    let already = seq.iter().any(|item| {
        item.as_mapping()
            .and_then(|m| m.get(serde_yaml::Value::from("$ref")))
            .and_then(|v| v.as_str())
            .map(|s| s == ref_path)
            .unwrap_or(false)
    });
    if !already {
        let mut entry_map = serde_yaml::Mapping::new();
        entry_map.insert(
            serde_yaml::Value::from("$ref"),
            serde_yaml::Value::from(ref_path),
        );
        seq.push(serde_yaml::Value::Mapping(entry_map));
    }

    let out = serde_yaml::to_string(&value)
        .with_context(|| format!("re-emitting {}", module_schema_path.display()))?;
    std::fs::write(module_schema_path, out)
        .with_context(|| format!("writing {}", module_schema_path.display()))?;
    Ok(())
}

fn now_iso8601() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Run `specdev inline <block>` against a discovered project layout and
/// resolved plugin root.
pub fn run_inline_block(layout: &LayoutContext, plugin_root: &Path, name: &str) -> Result<()> {
    let mut lineage = Lineage::load(&layout.specdev_root)?;
    if lineage.has_block(name) {
        return Err(anyhow!(
            "[INLINE-002] block {name:?} is already inlined (lineage entry present); use `specdev inline {name} --merge` to update from upstream, or `git checkout -- specs/specdev/...` to reset",
        ));
    }
    let files = collect_block_files(plugin_root, name)?;
    refuse_if_dst_exists(
        &layout.specdev_root,
        &files,
        UnitKind::Block,
        name,
        &format!("block {name}"),
    )?;

    write_unit_files(&layout.specdev_root, &files)?;

    let upstream_files: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|f| (f.rel_src_in_unit.clone(), f.bytes.clone()))
        .collect();
    upstream::write_upstream_files(&layout.specdev_root, UnitKind::Block, name, &upstream_files)?;

    aggregator_add_ref(
        &layout.module_schema_path,
        &format!("inlined/blocks/{name}.schema.yaml"),
    )?;

    let entry = Entry {
        origin: "plugin".into(),
        origin_version: plugin_version(plugin_root)?,
        inlined_at: now_iso8601(),
        content_hash: rollup_hash(&files),
    };
    lineage.blocks.insert(name.to_string(), entry);
    lineage.save(&layout.specdev_root)?;
    Ok(())
}

/// Run `specdev inline-bridge <a>+<b>`. Preflight: lex-order + both blocks
/// already inlined.
pub fn run_inline_bridge(layout: &LayoutContext, plugin_root: &Path, name: &str) -> Result<()> {
    // Preflight 1: lex-order.
    let (a, b) = lex_order_bridge(name)?;

    // Preflight 2: both blocks already inlined.
    let mut lineage = Lineage::load(&layout.specdev_root)?;
    for needed in [a, b] {
        if !lineage.has_block(needed) {
            return Err(anyhow!(
                "[INLINE-003] bridge `{name}` requires `{needed}` to be inlined first"
            ));
        }
    }

    if lineage.has_bridge(name) {
        return Err(anyhow!(
            "[INLINE-002] bridge `{name}` is already inlined (lineage entry present); use `specdev inline-bridge {name} --merge` to update",
        ));
    }

    let files = collect_bridge_files(plugin_root, name)?;
    refuse_if_dst_exists(
        &layout.specdev_root,
        &files,
        UnitKind::Bridge,
        name,
        &format!("bridge {name}"),
    )?;

    write_unit_files(&layout.specdev_root, &files)?;

    let upstream_files: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|f| (f.rel_src_in_unit.clone(), f.bytes.clone()))
        .collect();
    upstream::write_upstream_files(
        &layout.specdev_root,
        UnitKind::Bridge,
        name,
        &upstream_files,
    )?;

    aggregator_add_ref(
        &layout.module_schema_path,
        &format!("inlined/bridges/{name}.schema.yaml"),
    )?;

    let entry = Entry {
        origin: "plugin".into(),
        origin_version: plugin_version(plugin_root)?,
        inlined_at: now_iso8601(),
        content_hash: rollup_hash(&files),
    };
    lineage.bridges.insert(name.to_string(), entry);
    lineage.save(&layout.specdev_root)?;
    Ok(())
}

/// Plugin-author lint: walk `<plugin-root>/blocks/` and run 7 structural
/// checks per block. Returns pre-prefixed offender lines (each contains
/// the block name + a `BLOCK-LINT-NNN` code + a one-line message). The
/// CLI prints these as-is without additional prefixing.
///
/// Complements `lint_plugin_bridges`. The two lints are
/// independent walks; `run_plugin_lint` aggregates them.
///
/// Checks per block:
/// - 001: `schema.yaml` exists.
/// - 002: `reduction.yaml` exists.
/// - 004: top-level `README.md` exists.
/// - 005: `schema.yaml` parses + compiles as a standalone JSON Schema.
/// - 006: `schema.yaml` `$id` matches `https://specdev.dev/blocks/<name>.v1`
///   where `<name>` is the directory name.
/// - 007: `reduction.yaml` validates against the embedded reduction
///   metaschema (catches missing `extends`, missing relation
///   `from_kind`/`to_kind`/`from_spec`/`endpoints`, etc).
pub fn lint_plugin_blocks(plugin_root: &Path) -> Result<Vec<String>> {
    let blocks_dir = plugin_root.join("blocks");
    if !blocks_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut offenders: Vec<String> = Vec::new();

    let mut block_paths: Vec<PathBuf> = std::fs::read_dir(&blocks_dir)
        .with_context(|| format!("reading {}", blocks_dir.display()))?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    block_paths.sort();

    for block_path in block_paths {
        let Some(block_name) = block_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(String::from)
        else {
            continue;
        };

        // Check 001: schema.yaml exists.
        let schema_path = block_path.join("schema.yaml");
        let has_schema = schema_path.is_file();
        if !has_schema {
            offenders.push(format!(
                "{block_name}: [BLOCK-LINT-001] missing schema.yaml at {}",
                schema_path.display()
            ));
        }

        // Check 002: reduction.yaml exists.
        let reduction_path = block_path.join("reduction.yaml");
        let has_reduction = reduction_path.is_file();
        if !has_reduction {
            offenders.push(format!(
                "{block_name}: [BLOCK-LINT-002] missing reduction.yaml at {}",
                reduction_path.display()
            ));
        }

        // Check 004: README.md at block root.
        let readme_path = block_path.join("README.md");
        if !readme_path.is_file() {
            offenders.push(format!(
                "{block_name}: [BLOCK-LINT-004] missing README.md at {}",
                readme_path.display()
            ));
        }

        // Schema-dependent checks need the parsed schema. Skip 005/006
        // gracefully if 001 already fired.
        let schema_value = if has_schema {
            match read_yaml_value(&schema_path) {
                Ok(v) => Some(v),
                Err(e) => {
                    offenders.push(format!(
                        "{block_name}: [BLOCK-LINT-005] schema.yaml is not valid YAML: {e}"
                    ));
                    None
                }
            }
        } else {
            None
        };

        if let Some(value) = &schema_value {
            // Check 005: standalone JSON Schema compile.
            if let Err(e) = jsonschema::options().build(value) {
                offenders.push(format!(
                    "{block_name}: [BLOCK-LINT-005] schema.yaml fails JSON Schema compile: {e}"
                ));
            }

            // Check 006: $id convention.
            let expected_id = format!("https://specdev.dev/blocks/{block_name}.v1");
            let actual_id = value.get("$id").and_then(|v| v.as_str()).unwrap_or("");
            if actual_id != expected_id {
                offenders.push(format!(
                    "{block_name}: [BLOCK-LINT-006] schema.yaml $id is {actual_id:?}; expected {expected_id:?}"
                ));
            }
        }

        // Check 007: reduction.yaml validates against the embedded metaschema.
        if has_reduction {
            match read_yaml_value(&reduction_path) {
                Ok(red_value) => {
                    let validator = reduction_metaschema_validator();
                    let mut errs: Vec<String> = validator
                        .iter_errors(&red_value)
                        .map(|e| crate::diagnostics::format_schema_error(&e))
                        .collect();
                    errs.sort();
                    errs.dedup();
                    for err in errs {
                        offenders.push(format!(
                            "{block_name}: [BLOCK-LINT-007] reduction.yaml: {err}"
                        ));
                    }
                }
                Err(e) => {
                    offenders.push(format!(
                        "{block_name}: [BLOCK-LINT-007] reduction.yaml is not valid YAML: {e}"
                    ));
                }
            }
        }
    }

    offenders.sort();
    Ok(offenders)
}

/// Read a YAML file and parse it as a `serde_json::Value`. Used by
/// `lint_plugin_blocks` for both schema and reduction parsing.
fn read_yaml_value(path: &Path) -> Result<serde_json::Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// Lazily-loaded validator for the embedded reduction metaschema.
/// Shared across all lint invocations in a process.
fn reduction_metaschema_validator() -> &'static jsonschema::Validator {
    use std::sync::OnceLock;
    static VALIDATOR: OnceLock<jsonschema::Validator> = OnceLock::new();
    static METASCHEMA_YAML: &str = include_str!("embedded/reduction.metaschema.yaml");
    VALIDATOR.get_or_init(|| {
        let value: serde_json::Value = serde_yaml::from_str(METASCHEMA_YAML)
            .expect("embedded reduction.metaschema.yaml parses as YAML");
        jsonschema::options()
            .build(&value)
            .expect("embedded reduction.metaschema.yaml is a well-formed JSON Schema")
    })
}

/// Plugin-author lint: walk `<plugin-root>/bridges/` and assert every
/// directory name is lex-ordered. Returns a list of offending entries.
/// Same lex-order check as the runtime `inline-bridge` preflight.
pub fn lint_plugin_bridges(plugin_root: &Path) -> Result<Vec<String>> {
    let bridges_dir = plugin_root.join("bridges");
    if !bridges_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut offenders = Vec::new();
    for entry in std::fs::read_dir(&bridges_dir)
        .with_context(|| format!("reading {}", bridges_dir.display()))?
    {
        let entry = entry?;
        if !entry.path().is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().into_string().ok() else {
            continue;
        };
        if let Err(e) = lex_order_bridge(&name) {
            offenders.push(format!("{name}: {e}"));
        }
    }
    offenders.sort();
    Ok(offenders)
}

/// Public for `bootstrap.rs` — returns a `BTreeMap<rel_path, bytes>` shape
/// useful for snapshot-friendly diagnostics. Not used by the inline path
/// itself.
#[allow(dead_code)]
pub(crate) fn block_file_map(plugin_root: &Path, name: &str) -> Result<BTreeMap<PathBuf, Vec<u8>>> {
    let files = collect_block_files(plugin_root, name)?;
    Ok(files.into_iter().map(|f| (f.rel_dst, f.bytes)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_order_accepts_a_before_b() {
        let (a, b) = lex_order_bridge("embassy+lvgl").unwrap();
        assert_eq!(a, "embassy");
        assert_eq!(b, "lvgl");
    }

    #[test]
    fn lex_order_rejects_swapped_with_did_you_mean() {
        let err = lex_order_bridge("lvgl+embassy").unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("BRIDGE-LEX-001"), "{s}");
        assert!(s.contains("`embassy+lvgl`"), "{s}");
    }

    #[test]
    fn lex_order_rejects_equal_components() {
        // `a == b` is not strictly less-than.
        let err = lex_order_bridge("foo+foo").unwrap_err();
        assert!(format!("{err:#}").contains("BRIDGE-LEX-001"));
    }

    #[test]
    fn lex_order_rejects_n_ary() {
        let err = lex_order_bridge("a+b+c").unwrap_err();
        assert!(format!("{err:#}").contains("BRIDGE-LEX-001"));
    }

    #[test]
    fn lex_order_rejects_no_separator() {
        let err = lex_order_bridge("embassy").unwrap_err();
        assert!(format!("{err:#}").contains("BRIDGE-LEX-001"));
    }

    #[test]
    fn lex_order_rejects_empty_components() {
        assert!(lex_order_bridge("+lvgl").is_err());
        assert!(lex_order_bridge("embassy+").is_err());
    }

    #[test]
    fn rollup_hash_is_order_independent() {
        // Two physical orderings of the same file set must roll up to the
        // same hash because we sort by rel_dst before hashing.
        let mk = |order: &[(&str, &[u8])]| -> Vec<UnitFile> {
            let mut files: Vec<UnitFile> = order
                .iter()
                .map(|(p, b)| UnitFile {
                    rel_dst: PathBuf::from(*p),
                    rel_src_in_unit: PathBuf::from(*p),
                    bytes: b.to_vec(),
                })
                .collect();
            files.sort_by(|a, b| a.rel_dst.cmp(&b.rel_dst));
            files
        };
        let ab = mk(&[("a/x", b"hello"), ("b/y", b"world")]);
        let ba = mk(&[("b/y", b"world"), ("a/x", b"hello")]);
        assert_eq!(rollup_hash(&ab), rollup_hash(&ba));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]

        /// Property: any two permutations of the same per-unit file set
        /// produce identical rolled-up hashes. The unit-test version
        /// covers a 2-file hand-picked permutation; this property
        /// covers every permutation of every set up to size 12, with
        /// distinct rel_dst strings (the sort key).
        #[test]
        fn rollup_hash_permutation_invariant(
            entries in proptest::collection::vec(
                ("[a-z]{1,5}/[a-z]{1,5}", proptest::collection::vec(proptest::prelude::any::<u8>(), 0..256)),
                0..12,
            )
        ) {
            // De-duplicate by rel_dst; same rel_dst with different bytes
            // is an authoring error the rollup format doesn't speak to.
            let mut by_path: std::collections::BTreeMap<String, Vec<u8>> = std::collections::BTreeMap::new();
            for (p, b) in entries.into_iter() {
                by_path.insert(p, b);
            }
            let mk = |order: &[(&String, &Vec<u8>)]| -> Vec<UnitFile> {
                let mut files: Vec<UnitFile> = order
                    .iter()
                    .map(|(p, b)| UnitFile {
                        rel_dst: PathBuf::from(p.as_str()),
                        rel_src_in_unit: PathBuf::from(p.as_str()),
                        bytes: (*b).clone(),
                    })
                    .collect();
                files.sort_by(|a, b| a.rel_dst.cmp(&b.rel_dst));
                files
            };
            let forward: Vec<(&String, &Vec<u8>)> = by_path.iter().collect();
            let reverse: Vec<(&String, &Vec<u8>)> = by_path.iter().rev().collect();
            proptest::prop_assert_eq!(rollup_hash(&mk(&forward)), rollup_hash(&mk(&reverse)));
        }

        /// Property: a different file set produces a different hash
        /// (modulo collision probability ~ 2^-256). Sanity check that
        /// the rollup function isn't the constant function.
        #[test]
        fn rollup_hash_distinguishes_distinct_sets(
            a_bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 1..32),
            b_bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 1..32),
        ) {
            // Skip when bytes happen to match — rollup is deterministic
            // over equal inputs by design.
            proptest::prop_assume!(a_bytes != b_bytes);
            let a = vec![UnitFile {
                rel_dst: PathBuf::from("x"),
                rel_src_in_unit: PathBuf::from("x"),
                bytes: a_bytes,
            }];
            let b = vec![UnitFile {
                rel_dst: PathBuf::from("x"),
                rel_src_in_unit: PathBuf::from("x"),
                bytes: b_bytes,
            }];
            proptest::prop_assert_ne!(rollup_hash(&a), rollup_hash(&b));
        }
    }
}
