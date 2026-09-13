//! Read/write `specs/specdev/gen-lineage.toml` + the gen-drift report.
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
//! content_hash = "sha256:..."     # of the emitted file's bytes (output side)
//! input_hash   = "sha256:..."     # of the module's whole assembled prompt (input side)
//! ```
//!
//! Keys are project-root-relative paths with forward slashes. Re-recording
//! a path replaces its entry — the file always reflects the latest gen run.
//! Byte-stability via `BTreeMap`, same discipline as [`crate::lineage`].
//!
//! A second table records **compile-gate results**, because a gate result
//! is about a module (or the whole tree), not a file, and so cannot live on
//! a per-file entry:
//!
//! ```toml
//! [[gates]]
//! module = "Dsp"                  # absent for a batch (whole-tree) row
//! label  = "cargo-check"          # opaque to the analyzer; the skill names it
//! pass   = true
//! key    = "sha256:...|sha256:..." # input_hash ⊕ lockfile/toolchain hash
//! ```
//!
//! One row per `(module, label)`; a new result replaces the old row, and
//! rows carry no timestamp — this file is committed and must not churn.
//! The `key` binds a result to the inputs it was about: the module's
//! `input_hash` (what was generated) and a hash of every lockfile and
//! toolchain-pin file the language adapter names (what it was compiled
//! against). A spec edit or a `cargo update` therefore invalidates a pass
//! without anyone deleting it. A pass row means exactly: one environment
//! compiled these inputs against this lockfile, once. The toolchain
//! *version* is deliberately not in the key — it is a machine fact, and
//! this file is committed.
//!
//! `report gen-plan` reads the table to add a second axis to its verdicts:
//! `verified` (a pass row exists at the current key). The skill re-gates
//! `unchanged && !verified` modules — running the compiler, not the LLM.
//!
//! The `content_hash` witnesses the *output* (did the emitted file change?);
//! the `input_hash` witnesses the *input* (did the generation prompt change?).
//! Together they let a later run skip a module whose inputs are unchanged and
//! whose output is still clean — incremental `/spec-gen --all` regeneration.
//! `input_hash` is absent for project-level files (no module) and whenever the
//! module's prompt could not be assembled at record time (fail-safe: a module
//! with no recorded `input_hash` cannot be proven unchanged, so it regenerates).
//!
//! Scope fence: this is a hash-level tripwire only. It detects THAT
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
    /// Compile-gate results, one per `(module, label)`. Kept sorted by
    /// `(module, label)` on save so the file is byte-stable. Absent from
    /// lineage files written before gate recording existed — those load
    /// with an empty table and every module reads unverified once.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<GateResult>,
}

/// One compile-gate result. See the module docs for the key's meaning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GateResult {
    /// Module the gate was recorded for. Absent for a batch row — the
    /// whole-tree result of the run (the thing `cargo check` actually
    /// proves; per-module Rust rows are the under-claiming projection).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Opaque gate label chosen by the skill (`cargo-check`, `flutter-analyze`,
    /// `next-build`). The analyzer never enumerates or interprets labels.
    pub label: String,
    pub pass: bool,
    /// `<input_hash or "none">|<lockfile+toolchain hash or "none">`. A batch
    /// row has no module and so no input part: `none|<lock hash>`.
    pub key: String,
}

impl GateResult {
    /// Sort key: batch rows (no module) first, then by module, then label.
    fn order(&self) -> (Option<&str>, &str) {
        (self.module.as_deref(), self.label.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenEntry {
    /// Module the file was generated for. Absent for project-level
    /// emissions (Cargo.toml).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    pub generated_at: String,
    /// SHA-256 of the emitted file's bytes — the output side of the tripwire.
    pub content_hash: String,
    /// SHA-256 of the module's whole assembled codegen prompt — the input
    /// side. Same value across every file recorded for one module. Absent
    /// for project-level files (no module) and when the prompt could not be
    /// assembled at record time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<String>,
    /// `true` when the file was derived by `derive --write` from a
    /// profile `derived_files` rule, not emitted by the codegen skill.
    /// A derived row is owned by `derive`: `gen-record` refuses it, a
    /// missing or edited derived file never changes its module's
    /// `gen-plan` verdict, and a derived row no rule produces any more is
    /// stale (file and row removed when the bytes still match). Absent
    /// (`false`) in lineage files written before derived files existed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub derived: bool,
}

/// Ownership rule for a lineage path, shared by `gen-record` and
/// `derive`: a path already recorded under a **different module that
/// still exists in the spec set** belongs to that module and is refused.
/// A stale owner (a deleted or renamed module) is reassigned; the caller
/// gets the old name back to print a note. `None` when nothing changes
/// hands. `writer` names the command in the error.
pub(crate) fn check_ownership(
    existing: Option<&GenEntry>,
    key: &str,
    new_module: Option<&str>,
    live_modules: &[String],
    writer: &str,
) -> Result<Option<String>> {
    let Some(row) = existing else {
        return Ok(None);
    };
    let Some(old) = row.module.as_deref() else {
        // A project-level row has no module to be alive; the new writer
        // takes it.
        return Ok(None);
    };
    if Some(old) == new_module {
        return Ok(None);
    }
    if live_modules.iter().any(|m| m == old) {
        bail!(
            "{writer}: `{key}` is already recorded for module `{old}`, which still \
             exists in specs/modules/; one path has one owner. Remove the file \
             from `{old}`'s emission (or delete that module's spec) before \
             recording it for {}",
            match new_module {
                Some(m) => format!("module `{m}`"),
                None => "the project".to_string(),
            }
        );
    }
    Ok(Some(old.to_string()))
}

impl GenLineage {
    /// Load from `<specdev_root>/gen-lineage.toml`. Returns an empty
    /// record if the file does not exist (no gen run recorded yet).
    pub fn load(specdev_root: &Path) -> Result<Self> {
        let path = gen_lineage_path(specdev_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
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
        let mut sorted = self.clone();
        sorted.gates.sort_by(|a, b| a.order().cmp(&b.order()));
        toml::to_string(&sorted).context("serializing gen-lineage")
    }

    /// Insert or replace the row for `(module, label)`.
    pub fn upsert_gate(&mut self, row: GateResult) {
        match self
            .gates
            .iter_mut()
            .find(|g| g.module == row.module && g.label == row.label)
        {
            Some(existing) => *existing = row,
            None => self.gates.push(row),
        }
    }

    /// The recorded result for `(module, label)`, if any.
    pub fn gate(&self, module: Option<&str>, label: &str) -> Option<&GateResult> {
        self.gates
            .iter()
            .find(|g| g.module.as_deref() == module && g.label == label)
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
    record_files_inner(layout, module, files, generated_at, false)
}

/// `record_files` with the edited-file guard lifted.
///
/// Use only when the caller has decided the current bytes are the intended
/// baseline — a hand-edit deliberately kept, or a formatter run that is part
/// of emission. Everything else must go through `record_files`.
pub fn record_files_accepting_edits(
    layout: &LayoutContext,
    module: Option<&str>,
    files: &[PathBuf],
    generated_at: &str,
) -> Result<Vec<String>> {
    record_files_inner(layout, module, files, generated_at, true)
}

fn record_files_inner(
    layout: &LayoutContext,
    module: Option<&str>,
    files: &[PathBuf],
    generated_at: &str,
    accept_edits: bool,
) -> Result<Vec<String>> {
    if files.is_empty() {
        bail!("gen-record: no files given — pass the emitted file paths");
    }

    // Guard: refuse to overwrite the baseline of a file that already drifted.
    //
    // Recording is how a file becomes "clean", so an unguarded record after a
    // hand-edit makes that edit the new truth and gen-drift can never surface
    // it again. Observed in the field: edit → gen-record → `clean` forever,
    // with the edit still in the source. The tripwire reported healthy while
    // the code no longer matched what `/spec-gen` produced.
    //
    // A file that is `clean` or absent from the lineage records freely — the
    // common path (a fresh emission) is unaffected.
    if !accept_edits {
        let drifted: Vec<String> = run_gen_drift(layout)?
            .files
            .into_iter()
            .filter(|d| d.category == GenFileCategory::Edited)
            .map(|d| d.path)
            .collect();
        if !drifted.is_empty() {
            let requested: std::collections::BTreeSet<String> = files
                .iter()
                .filter_map(|f| f.canonicalize().ok())
                .filter_map(|abs| {
                    let root = layout.project_root.canonicalize().ok()?;
                    abs.strip_prefix(&root).ok().map(rel_key)
                })
                .collect();
            let collisions: Vec<&String> =
                drifted.iter().filter(|p| requested.contains(*p)).collect();
            if !collisions.is_empty() {
                bail!(
                    "gen-record: refusing to re-record {} file(s) that gen-drift \
                     reports as `edited`:\n{}\n\n\
                     Recording is what makes a file `clean`, so recording now would \
                     make the hand-edit the new baseline and gen-drift could never \
                     report it again.\n\
                     Either re-run `/spec-gen` for the module so the emitted bytes \
                     match the spec, or fold the edit into the spec and regenerate. \
                     If the current bytes ARE the intended baseline (a kept edit, or \
                     a formatter pass that belongs to emission), pass --accept-edits.",
                    collisions.len(),
                    collisions
                        .iter()
                        .map(|p| format!("  - {p}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
            }
        }
    }
    let project_root = layout
        .project_root
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", layout.project_root.display()))?;

    // Compute the input-side hash once per module — it is the same whole
    // assembled prompt for every file this module emitted. Best-effort: a
    // module that can no longer assemble a prompt (e.g. its spec was removed
    // between gen and record) still records its output hashes, but with no
    // input_hash, so the next incremental run treats it as stale rather than
    // skipping it on a hash it could not compute.
    let input_hash = match module {
        Some(name) => match crate::prompt::module_input_hash(layout, name) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::warn!(
                    target: "specdev_analyzer::gen_lineage",
                    "could not assemble the prompt for module `{name}` to record its \
                     input_hash ({e:#}); it will be treated as stale on the next \
                     incremental `/spec-gen --all`"
                );
                None
            }
        },
        None => None,
    };

    let mut lineage = GenLineage::load(&layout.specdev_root)?;
    let live_modules = crate::prompt::list_module_names(layout)?;
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
        let existing = lineage.files.get(&key);
        // A derived row is `derive`'s: a plain record would drop the flag
        // and stale deletion would stop for that file.
        if existing.is_some_and(|e| e.derived) {
            bail!(
                "gen-record: `{key}` is a derived file (written by `specdev-analyzer \
                 derive` from a profile `derived_files` rule); it is never listed \
                 for gen-record. Run `derive --write` to refresh it."
            );
        }
        if let Some(old) = check_ownership(existing, &key, module, &live_modules, "gen-record")? {
            tracing::warn!(
                target: "specdev_analyzer::gen_lineage",
                "gen-record: `{key}` was recorded for module `{old}`, which no longer \
                 exists in specs/modules/; reassigned to {}",
                module.map(|m| format!("module `{m}`")).unwrap_or_else(|| "the project".into())
            );
        }
        let bytes = std::fs::read(&abs).with_context(|| format!("reading {}", abs.display()))?;
        lineage.files.insert(
            key.clone(),
            GenEntry {
                module: module.map(str::to_string),
                generated_at: generated_at.to_string(),
                content_hash: content_hash(&bytes),
                input_hash: input_hash.clone(),
                derived: false,
            },
        );
        recorded.push(key);
    }
    lineage.save(&layout.specdev_root)?;
    recorded.sort();
    Ok(recorded)
}

// ---------------------------------------------------------------------------
// Compile-gate results.
// ---------------------------------------------------------------------------

/// Placeholder for an absent key component. A missing lockfile is a real,
/// stable state (a fresh crate before its first `cargo check`; a resolution
/// failure that never wrote one) and must key deterministically — it is
/// not an error and not the same as any hash.
pub const KEY_NONE: &str = "none";

/// Hash of every lockfile and toolchain-pin file the project's language
/// adapter names, found on a walk **up** from the project root.
///
/// The walk is bounded by, in order of preference: the directory of the
/// workspace `spec.toml` that lists this project as a member; else the
/// nearest ancestor holding a `.git` entry (repository root — a lockfile
/// outside the repository cannot be the project's); else the project root
/// itself (no walk). A cargo workspace keeps one `Cargo.lock` at its root
/// and *none* in its members, so the walk is what lets a member's gate key
/// see its real lockfile. Every candidate found on the walk is hashed —
/// over-inclusion only costs a re-gate.
///
/// Returns [`KEY_NONE`] when nothing is found. Call this **after** the
/// gate, never before: `cargo check` creates or rewrites `Cargo.lock`, and
/// a package-manager install step rewrites its lockfile mid-gate.
pub fn lockfile_hash(layout: &LayoutContext) -> Result<String> {
    let language = layout.spec.project.language.as_str();
    let adapter = crate::lang::adapter_for(language)
        .ok_or_else(|| anyhow!("gen-record-gate: unknown language `{language}`"))?;
    let mut names: Vec<&str> = adapter.lockfile_candidates().to_vec();
    names.extend_from_slice(adapter.toolchain_file_candidates());
    if names.is_empty() {
        return Ok(KEY_NONE.to_string());
    }

    let root = layout
        .project_root
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", layout.project_root.display()))?;
    let bound = gate_walk_bound(&root)?;

    // Collect (relative-name, bytes) for every candidate found between the
    // root and the bound, inclusive. Sorted by path so the hash is stable.
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    let mut cursor: Option<&Path> = Some(root.as_path());
    while let Some(dir) = cursor {
        for name in &names {
            let p = dir.join(name);
            if p.is_file() {
                let bytes =
                    std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?;
                found.push((p.to_string_lossy().into_owned(), bytes));
            }
        }
        if dir == bound {
            break;
        }
        cursor = dir.parent();
    }
    if found.is_empty() {
        return Ok(KEY_NONE.to_string());
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buf: Vec<u8> = Vec::new();
    for (path, bytes) in &found {
        // Key on the file name, not the absolute path — the same lockfile
        // must hash identically on every checkout of the repository.
        let name = Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(bytes);
        buf.push(0);
    }
    Ok(content_hash(&buf))
}

/// Where the lockfile walk stops (inclusive). See [`lockfile_hash`].
fn gate_walk_bound(root: &Path) -> Result<PathBuf> {
    if let Some(ws_spec) = crate::layout::find_workspace_root_spec(root)? {
        if let Some(dir) = ws_spec.parent() {
            return Ok(dir.to_path_buf());
        }
    }
    let mut cursor = root;
    loop {
        if cursor.join(".git").exists() {
            return Ok(cursor.to_path_buf());
        }
        match cursor.parent() {
            Some(p) => cursor = p,
            None => return Ok(root.to_path_buf()),
        }
    }
}

/// Compose a gate key from its three parts. `first` is the row's own
/// input half — a module row's `input_hash`, a batch row's
/// [`modules_hash`] — joined with the two run-level hashes that key every
/// row.
pub fn compose_gate_key(first: Option<&str>, lock_hash: &str, profile_hash: &str) -> String {
    format!("{}|{lock_hash}|{profile_hash}", first.unwrap_or(KEY_NONE))
}

/// Hash of the profile substrate the gate ran against: the inlined
/// `profile.yaml`, every file under `<specdev_root>/templates/`
/// (**recursively** — bootstrap copies flat today and the hash must not
/// assume it), and every declared `paired_files` target that exists.
///
/// Why: `input_hash` hashes the assembled module prompt, which carries no
/// derived-file rule, no shim template and no gate configuration. A
/// template edit rewrites every derived file while every module stays
/// `unchanged` and `verified`, and nothing re-gates; a `strict: false`
/// flip in `tsconfig.json` changes what the gate proves. Every file
/// hashed here travels with the repository.
///
/// Returns [`KEY_NONE`] when none of the three exists — the same
/// convention as [`lockfile_hash`]. Call this **after** the gate: a first
/// `next build` rewrites `tsconfig.json`, which is a paired target.
///
/// Limit: gate configuration outside `specs/specdev/` and outside
/// `paired_files` (`.cargo/config.toml`, `analysis_options.yaml`) is not
/// in the key. Over-inclusion costs one re-gate; under-inclusion hides a
/// change, so the direction is deliberate.
pub fn profile_hash(layout: &LayoutContext) -> Result<String> {
    // (key, bytes). The key is a stable logical name, never an absolute
    // path: the same tree must hash identically on every checkout. The
    // three prefixes keep a paired target named `profile.yaml` distinct
    // from the profile itself.
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();

    let profile_path = layout.specdev_root.join("profile.yaml");
    if profile_path.is_file() {
        found.push(("profile.yaml".to_string(), read_bytes(&profile_path)?));
    }

    let templates_dir = layout.specdev_root.join("templates");
    for path in walk_files(&templates_dir)? {
        let rel = path
            .strip_prefix(&templates_dir)
            .map(rel_key)
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());
        found.push((format!("templates/{rel}"), read_bytes(&path)?));
    }

    for target in crate::paired_files::paired_targets(layout)? {
        let abs = layout.project_root.join(&target);
        if abs.is_file() {
            found.push((format!("paired/{}", rel_key(&target)), read_bytes(&abs)?));
        }
    }

    if found.is_empty() {
        return Ok(KEY_NONE.to_string());
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buf: Vec<u8> = Vec::new();
    for (key, bytes) in &found {
        buf.extend_from_slice(key.as_bytes());
        buf.push(0);
        buf.extend_from_slice(bytes);
        buf.push(0);
    }
    Ok(content_hash(&buf))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

/// Every file under `dir`, recursively, sorted by path. An absent
/// directory is an empty list, not an error.
///
/// A **symlinked directory is not descended into**. A link back to an
/// ancestor is a cycle: measured on macOS the walk re-enters the
/// directory until the OS refuses the deep path (`ELOOP` at 32 levels),
/// which ends the walk quietly with the same file hashed thirty-odd
/// times — a hash nobody can reproduce; on a platform with a looser
/// limit it need not end at all. A symlinked *file* cannot loop and is
/// read like any other.
fn walk_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in
            std::fs::read_dir(&current).with_context(|| format!("reading {}", current.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            // `file_type()` does not follow the link; `is_file()` does,
            // so a symlink to a file is still hashed.
            if entry.file_type()?.is_symlink() {
                if path.is_file() {
                    out.push(path);
                }
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Hash over the whole spec set as `(module, input_hash)` pairs, sorted
/// by module name. A module whose prompt cannot be assembled contributes
/// the fixed token [`KEY_NONE`] — unassemblable is a state, not an error,
/// and it must key deterministically.
///
/// This is the input half of a **batch** row's key. Without it a
/// `next build` row keyed on the lockfile alone reads `current: true`
/// after any module regenerates, and the report says the tree builds when
/// it was never built with the new code.
pub fn modules_hash(layout: &LayoutContext) -> Result<String> {
    Ok(modules_hash_from(&module_input_hashes(layout)?))
}

/// Freshly assembled input hash per module in the spec set, in module
/// order. `None` for a module whose prompt cannot be assembled.
pub fn module_input_hashes(layout: &LayoutContext) -> Result<Vec<(String, Option<String>)>> {
    let names = crate::prompt::list_module_names(layout)?;
    Ok(names
        .into_iter()
        .map(|name| {
            let hash = crate::prompt::module_input_hash_quiet(layout, &name).ok();
            (name, hash)
        })
        .collect())
}

/// [`modules_hash`] over pairs a caller already assembled. `run_gen_plan`
/// needs every module's input hash for its own verdict anyway; assembling
/// twice would cost one whole prompt build per module.
pub fn modules_hash_from(inputs: &[(String, Option<String>)]) -> String {
    let mut sorted: Vec<&(String, Option<String>)> = inputs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut buf: Vec<u8> = Vec::new();
    for (name, hash) in sorted {
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        buf.extend_from_slice(hash.as_deref().unwrap_or(KEY_NONE).as_bytes());
        buf.push(0);
    }
    content_hash(&buf)
}

/// The run-level halves of a gate key. Compute once per run and **after**
/// the gate ran: the gate rewrites the lockfile (`cargo check` creates
/// `Cargo.lock`; an install step rewrites its own) and a first
/// `next build` rewrites a paired target.
#[derive(Debug, Clone)]
pub struct GateKeyBasis {
    /// [`lockfile_hash`] — keys every row.
    pub lock: String,
    /// [`profile_hash`] — keys every row.
    pub profile: String,
    /// [`modules_hash`] — the input half of a batch row.
    pub modules: String,
}

impl GateKeyBasis {
    /// Compute all three, assembling every module prompt once.
    pub fn compute(layout: &LayoutContext) -> Result<Self> {
        Self::from_inputs(layout, &module_input_hashes(layout)?)
    }

    /// [`GateKeyBasis::compute`] reusing input hashes the caller has.
    pub fn from_inputs(
        layout: &LayoutContext,
        inputs: &[(String, Option<String>)],
    ) -> Result<Self> {
        Ok(Self {
            lock: lockfile_hash(layout)?,
            profile: profile_hash(layout)?,
            modules: modules_hash_from(inputs),
        })
    }

    /// The key a batch row carries: the module set, the lockfile and the
    /// profile substrate.
    pub fn batch_key(&self) -> String {
        compose_gate_key(Some(&self.modules), &self.lock, &self.profile)
    }

    /// The key a module row carries, from that module's input hash
    /// (`None` when its prompt cannot be assembled).
    pub fn module_key(&self, input_hash: Option<&str>) -> String {
        compose_gate_key(input_hash, &self.lock, &self.profile)
    }
}

impl GateKeyBasis {
    /// The key a row for `module` carries right now. `inputs` is the
    /// same list the basis was built from; a module the spec set does not
    /// hold (only `gen-record-gate` can name one) falls back to a fresh
    /// assembly, which fails and keys on the fixed token.
    fn module_key_for(
        &self,
        layout: &LayoutContext,
        inputs: &[(String, Option<String>)],
        module: &str,
    ) -> String {
        match inputs.iter().find(|(name, _)| name == module) {
            Some((_, hash)) => self.module_key(hash.as_deref()),
            None => {
                let hash = crate::prompt::module_input_hash_quiet(layout, module).ok();
                self.module_key(hash.as_deref())
            }
        }
    }
}

/// What one gate run changed in `gen-lineage.toml`.
#[derive(Debug, Clone, Default)]
pub struct GateRecord {
    /// Rows written, batch row first.
    pub written: Vec<GateResult>,
    /// Rows removed because the registry no longer declares their label.
    /// Empty unless the caller passed a declared-label set.
    pub pruned: Vec<GateResult>,
}

/// Record a compile-gate result. Writes one row per module in `modules`
/// **and** one batch row (no module) for the run — the batch row is the
/// whole-tree fact the gate actually established; the module rows are the
/// under-claiming projection onto the modules this run generated. Rows
/// replace by `(module, label)`. The key is hashed here, i.e. **after**
/// the gate ran. Returns the rows written, batch row first.
pub fn record_gate(
    layout: &LayoutContext,
    modules: &[String],
    label: &str,
    pass: bool,
) -> Result<Vec<GateResult>> {
    Ok(record_gate_run(layout, modules, label, pass, None)?.written)
}

/// [`record_gate`] with the runner's extra job: when `declared` is
/// `Some`, every row whose label the registry no longer declares is
/// removed in the same save. A label rename would otherwise leave dead
/// rows in committed lineage forever — `gen-plan` already ignores them,
/// so nothing but the file itself notices. `gen-record-gate` passes
/// `None`: it is the manual command and knows no registry.
pub fn record_gate_run(
    layout: &LayoutContext,
    modules: &[String],
    label: &str,
    pass: bool,
    declared: Option<&[String]>,
) -> Result<GateRecord> {
    if label.trim().is_empty() {
        bail!("gen-record-gate: --label must not be empty");
    }
    // One prompt assembly per module for the whole record: the module
    // rows and the batch row's `modules_hash` read the same list.
    let inputs = module_input_hashes(layout)?;
    let basis = GateKeyBasis::from_inputs(layout, &inputs)?;
    let mut lineage = GenLineage::load(&layout.specdev_root)?;
    let mut out = GateRecord::default();

    if let Some(declared) = declared {
        let (keep, drop): (Vec<GateResult>, Vec<GateResult>) = lineage
            .gates
            .drain(..)
            .partition(|g| declared.iter().any(|l| l == &g.label));
        lineage.gates = keep;
        out.pruned = drop;
    }

    let batch = GateResult {
        module: None,
        label: label.to_string(),
        pass,
        key: basis.batch_key(),
    };
    lineage.upsert_gate(batch.clone());
    out.written.push(batch);

    let mut names: Vec<&String> = modules.iter().collect();
    names.sort();
    names.dedup();
    for name in names {
        let row = GateResult {
            module: Some(name.clone()),
            label: label.to_string(),
            pass,
            key: basis.module_key_for(layout, &inputs, name),
        };
        lineage.upsert_gate(row.clone());
        out.written.push(row);
    }
    lineage.save(&layout.specdev_root)?;
    Ok(out)
}

/// Project-root-relative key with forward slashes on every platform.
pub(crate) fn rel_key(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `rel` — a project-root-relative path — carries a row in
/// `gen-lineage.toml`.
///
/// The question is narrow and deliberate: **did this tool write the file that
/// is standing at that path?** A `false` on a file that exists means the file
/// is the user's, not an output of ours. That is the same fact `derive`
/// reports as `conflict: unrecorded file at target` — see [`crate::derive`].
///
/// A missing `gen-lineage.toml` yields `false` for every path, which is the
/// correct answer: a project that never ran `/spec-gen` owns everything in
/// its own tree.
pub fn is_recorded(specdev_root: &Path, rel: &Path) -> Result<bool> {
    Ok(GenLineage::load(specdev_root)?
        .files
        .contains_key(&rel_key(rel)))
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
    /// Whether `gen-lineage.toml` records anything at all.
    ///
    /// An empty report is ambiguous without this: `files: []` reads as "every
    /// generated file is clean" when it can equally mean "nothing was ever
    /// generated, so this command has no opinion about your tree". The second
    /// is the normal state of a project that adopted specdev over existing
    /// code, and reading it as the first is how a hand-written tree looks
    /// audited when it is not.
    pub nothing_recorded: bool,
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

/// Classify a single recorded file against its current bytes on disk.
/// Shared by the gen-drift report and the gen-plan classifier.
fn classify_recorded_file(
    project_root: &Path,
    rel: &str,
    recorded_hash: &str,
) -> Result<GenFileCategory> {
    let abs = project_root.join(rel);
    match std::fs::read(&abs) {
        Ok(bytes) => Ok(if content_hash(&bytes) == recorded_hash {
            GenFileCategory::Clean
        } else {
            GenFileCategory::Edited
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GenFileCategory::Missing),
        Err(e) => Err(anyhow!("reading {}: {e}", abs.display())),
    }
}

/// Classify every recorded file against its current bytes. Pure reporter:
/// never modifies files or the record.
pub fn run_gen_drift(layout: &LayoutContext) -> Result<GenDriftReport> {
    let lineage = GenLineage::load(&layout.specdev_root)?;
    let mut report = GenDriftReport {
        nothing_recorded: lineage.files.is_empty(),
        ..Default::default()
    };
    for (path, entry) in &lineage.files {
        let category = classify_recorded_file(&layout.project_root, path, &entry.content_hash)?;
        report.files.push(GenFileDrift {
            path: path.clone(),
            category,
            module: entry.module.clone(),
            generated_at: entry.generated_at.clone(),
        });
    }
    Ok(report)
}

/// Per-module verdict for incremental `/spec-gen --all`: whether a module's
/// generation can be skipped or must be regenerated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenPlanCategory {
    /// No gen-lineage entry for the module, and nothing at its declared
    /// `source_file` either — genuinely never generated. Regenerate.
    New,
    /// No gen-lineage entry, but the module's declared `source_file` already
    /// exists on disk. This tool did not write it, so it is hand-authored:
    /// a project that adopted specdev over a tree it already had.
    ///
    /// Distinct from `New` because the action is different. `New` means
    /// "emit it"; this means "ask first" — regenerating would overwrite
    /// working code the plan never mentioned. Same fact `derive` reports as
    /// `conflict: unrecorded file at target`, and the same one
    /// `manifest --write` refuses on ([`MANIFEST-003`]).
    ///
    /// [`MANIFEST-003`]: crate::gen_lineage::is_recorded
    Unrecorded,
    /// Output clean AND the recorded `input_hash` matches the freshly
    /// assembled prompt. Inputs unchanged → skip.
    Unchanged,
    /// Output clean but the generation inputs moved (the recorded
    /// `input_hash` differs from the current prompt, or was never
    /// recorded). Regenerate.
    Stale,
    /// At least one recorded output file was hand-edited. Skip + warn —
    /// never clobber a hand-edit; the user reconciles via gen-drift.
    Edited,
    /// At least one recorded output file is gone from disk. Regenerate.
    Missing,
}

impl GenPlanCategory {
    /// Whether `/spec-gen --all` regenerates this module. `unchanged` and
    /// `edited` are skipped (the latter to protect hand-edits); the rest
    /// regenerate.
    pub fn regenerates(self) -> bool {
        matches!(
            self,
            GenPlanCategory::New | GenPlanCategory::Stale | GenPlanCategory::Missing
        )
    }

    /// Whether this verdict needs a human decision before `/spec-gen` acts.
    ///
    /// `Edited` and `Unrecorded` both mean "there is work on disk that this
    /// tool did not put there". Neither regenerates; both must be surfaced
    /// rather than skipped silently, because a skipped module reads as done.
    pub fn needs_confirmation(self) -> bool {
        matches!(self, GenPlanCategory::Edited | GenPlanCategory::Unrecorded)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GenPlanModule {
    pub module: String,
    pub category: GenPlanCategory,
    /// Second axis, independent of `category`: does **every** module-scope
    /// gate the inlined profile declares have a **passing** row for this
    /// module at the *current* key (fresh `input_hash` ⊕ current lockfile
    /// hash)? `false` for a module that was never gated, failed a gate,
    /// passed only some of the declared gates, or whose spec or lockfile
    /// moved since it passed — and `false` for every module when no
    /// module-scope gate is declared (`gates_declared: false`). Rows
    /// under labels the registry no longer declares are ignored. The
    /// skill's re-gate set is `category == unchanged && !verified`: run
    /// the compile gate again, do not regenerate.
    pub verified: bool,
    /// Affected output files — populated for `edited` / `missing` /
    /// `unrecorded` so the skill can name them when it surfaces the module.
    /// Sorted; empty for the other categories.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// The whole-tree gate state, one per recorded batch label.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GenPlanBatchGate {
    pub label: String,
    pub pass: bool,
    /// Whether the row's key matches the current lockfile hash. A stale
    /// batch row says nothing about today's tree.
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct GenPlanReport {
    /// Whether the inlined `profile.yaml` declares at least one
    /// `scope: module` gate. When false, `verified` cannot be computed
    /// and reads `false` for every module — never `true` over zero
    /// gates. A report field only: the refusal lives in `gen --tail`,
    /// which runs in every codegen mode.
    pub gates_declared: bool,
    /// One verdict per module under `specs/modules/`, sorted by module name.
    pub modules: Vec<GenPlanModule>,
    /// Batch (no-module) gate rows, sorted by label. Empty when no batch
    /// gate was ever recorded. This is what tells the report whether the
    /// tree built the last time a gate ran — a module can be `verified`
    /// while a *later* module broke the build.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub batch: Vec<GenPlanBatchGate>,
}

impl GenPlanReport {
    /// Render as canonical YAML for the skill + snapshot tests.
    pub fn to_yaml(&self) -> Result<String> {
        serde_yaml::to_string(self).context("serializing gen-plan report")
    }
}

/// Classify every module under `specs/modules/` for incremental
/// regeneration. Deterministic — no Claude judgment: a module is `new`
/// (no lineage), `edited` (a recorded output was hand-edited), `missing`
/// (a recorded output is gone), `stale` (clean output, inputs moved), or
/// `unchanged` (clean output, inputs match). The `/spec-gen --all` skill
/// consumes this to skip `unchanged` + `edited` and regenerate the rest.
///
/// Category precedence when a module's files disagree: `edited` outranks
/// `missing` — any hand-edit means "don't auto-regenerate this module,"
/// since regeneration would clobber the edit (a still-missing file is the
/// lesser harm and is surfaced for reconciliation). Both outrank the
/// clean-output `stale`/`unchanged` split.
///
/// Project-level files (no module, e.g. Cargo.toml) are out of scope —
/// Cargo.toml emission is whole-project and fast, not per-module skippable.
pub fn run_gen_plan(layout: &LayoutContext) -> Result<GenPlanReport> {
    let lineage = GenLineage::load(&layout.specdev_root)?;
    let mut report = GenPlanReport::default();

    // The declared module-scope gate labels: the set a module must have
    // a current pass row for, from each, to read `verified`. An absent
    // inlined profile is "nothing declared", the same as an empty key.
    let declared: Vec<String> = crate::manifest::Profile::load_inlined(&layout.specdev_root)?
        .map(|p| p.module_gates().map(|g| g.label.clone()).collect())
        .unwrap_or_default();
    report.gates_declared = !declared.is_empty();

    // Every module's freshly assembled input hash, computed once. Three
    // readers need it: the module's own gate key, its `modules_hash` pair
    // in the batch key, and the input-side `unchanged` vs `stale` test.
    // Assembling a prompt is the expensive step in this command.
    let inputs = module_input_hashes(layout)?;

    // The run-level halves of the gate key, computed once per plan. Read
    // at plan time they reflect the last gate that ran (or a `cargo
    // update`, or a template edit, since) — exactly the thing a recorded
    // pass must be compared against.
    let basis = GateKeyBasis::from_inputs(layout, &inputs)?;

    // Batch rows: whole-tree state per label.
    let batch_key = basis.batch_key();
    let mut batch: Vec<GenPlanBatchGate> = lineage
        .gates
        .iter()
        .filter(|g| g.module.is_none())
        .map(|g| GenPlanBatchGate {
            label: g.label.clone(),
            pass: g.pass,
            current: g.key == batch_key,
        })
        .collect();
    batch.sort_by(|a, b| a.label.cmp(&b.label));
    report.batch = batch;

    for (name, input_hash) in &inputs {
        // Second axis: a passing gate row at the current key. Computed for
        // every category — a `new` or `stale` module is trivially
        // unverified (its key moved with its inputs), and saying so keeps
        // the field's meaning uniform.
        let current_key = basis.module_key(input_hash.as_deref());
        let verified = report.gates_declared
            && declared.iter().all(|label| {
                lineage.gates.iter().any(|g| {
                    g.module.as_deref() == Some(name.as_str())
                        && &g.label == label
                        && g.pass
                        && g.key == current_key
                })
            });

        // Files recorded for this module (project-level entries excluded).
        // Derived rows are excluded too: a shim is not evidence the module
        // was generated (a module with only derived rows reads `new`); a
        // missing derived file comes back for free on the next `derive`,
        // so it never makes the module `missing`; an edited derived file
        // is `derive`'s to report and never changes the verdict; and a
        // derived row's `input_hash` cannot be fresher than the body
        // row's (derive runs after emission in the same gen), so it adds
        // nothing to the input-side test.
        let entries: Vec<(&String, &GenEntry)> = lineage
            .files
            .iter()
            .filter(|(_, e)| e.module.as_deref() == Some(name.as_str()) && !e.derived)
            .collect();

        if entries.is_empty() {
            // No lineage row. Before calling this `new`, ask the question the
            // old code never asked: is there already a file at the path this
            // module declares? If there is, we did not write it, and
            // regenerating would overwrite hand-authored work that the plan
            // gave no warning about.
            //
            // Only an explicit `source_file` is consulted. When the spec does
            // not name one, the emission path is the codegen prose's to
            // decide and is not knowable here — that stays `new`, which is
            // the honest answer rather than a guess.
            let module_path = layout.modules_dir.join(format!("{name}.toml"));
            let declared = crate::items::source_map::load_source_file_override(&module_path);
            let existing: Vec<String> = declared
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .filter(|rel| layout.project_root.join(rel).is_file())
                .map(|rel| vec![rel_key(Path::new(rel))])
                .unwrap_or_default();

            let category = if existing.is_empty() {
                GenPlanCategory::New
            } else {
                GenPlanCategory::Unrecorded
            };
            report.modules.push(GenPlanModule {
                module: name.clone(),
                category,
                verified,
                files: existing,
            });
            continue;
        }

        let mut edited: Vec<String> = Vec::new();
        let mut missing: Vec<String> = Vec::new();
        for (path, entry) in &entries {
            match classify_recorded_file(&layout.project_root, path, &entry.content_hash)? {
                GenFileCategory::Clean => {}
                GenFileCategory::Edited => edited.push((*path).clone()),
                GenFileCategory::Missing => missing.push((*path).clone()),
            }
        }

        let (category, files) = if !edited.is_empty() {
            edited.sort();
            (GenPlanCategory::Edited, edited)
        } else if !missing.is_empty() {
            missing.sort();
            (GenPlanCategory::Missing, missing)
        } else {
            // All outputs clean — decide on the input side. The recorded
            // input_hash is uniform across a module's entries (one prompt →
            // one hash); require that uniformity, a recorded value, and a
            // match against the freshly assembled prompt. Any gap (no
            // recorded hash, non-uniform, or assembly failure → None) is
            // fail-safe `stale`: a module that cannot be proven unchanged
            // regenerates rather than being wrongly skipped.
            let first = entries[0].1.input_hash.clone();
            let uniform = entries.iter().all(|(_, e)| e.input_hash == first);
            let unchanged = uniform && first.is_some() && &first == input_hash;
            let category = if unchanged {
                GenPlanCategory::Unchanged
            } else {
                GenPlanCategory::Stale
            };
            (category, Vec::new())
        };

        report.modules.push(GenPlanModule {
            module: name.clone(),
            category,
            verified,
            files,
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
                input_hash: Some("sha256:in123".into()),
                derived: false,
            },
        );
        l.files.insert(
            "Cargo.toml".into(),
            GenEntry {
                module: None,
                generated_at: "2026-06-12T10:00:00Z".into(),
                content_hash: "sha256:def456".into(),
                input_hash: None,
                derived: false,
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
                        input_hash: None,
                        derived: false,
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
    fn module_and_input_hash_omitted_when_none() {
        let mut l = GenLineage::default();
        l.files.insert(
            "Cargo.toml".into(),
            GenEntry {
                module: None,
                generated_at: "2026-06-12T10:00:00Z".into(),
                content_hash: "sha256:abc".into(),
                input_hash: None,
                derived: false,
            },
        );
        let text = l.to_toml_string().unwrap();
        assert!(!text.contains("module"), "{text}");
        assert!(!text.contains("input_hash"), "{text}");
    }

    #[test]
    fn input_hash_serializes_when_present() {
        let mut l = GenLineage::default();
        l.files.insert(
            "src/dsp.rs".into(),
            GenEntry {
                module: Some("Dsp".into()),
                generated_at: "2026-06-12T10:00:00Z".into(),
                content_hash: "sha256:abc".into(),
                input_hash: Some("sha256:in".into()),
                derived: false,
            },
        );
        let text = l.to_toml_string().unwrap();
        assert!(text.contains("input_hash = \"sha256:in\""), "{text}");
    }
}
