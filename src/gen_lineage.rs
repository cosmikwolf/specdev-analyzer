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
        let bytes = std::fs::read(&abs).with_context(|| format!("reading {}", abs.display()))?;
        lineage.files.insert(
            key.clone(),
            GenEntry {
                module: module.map(str::to_string),
                generated_at: generated_at.to_string(),
                content_hash: content_hash(&bytes),
                input_hash: input_hash.clone(),
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

/// Compose a gate key from its two parts.
pub fn compose_gate_key(input_hash: Option<&str>, lock_hash: &str) -> String {
    format!("{}|{}", input_hash.unwrap_or(KEY_NONE), lock_hash)
}

/// The key a gate result for `module` would carry *right now*: the module's
/// freshly assembled `input_hash` (or `none` if the prompt cannot be
/// assembled, or for a batch row) joined with `lock_hash`.
pub fn current_gate_key(layout: &LayoutContext, module: Option<&str>, lock_hash: &str) -> String {
    let input = module.and_then(|m| crate::prompt::module_input_hash(layout, m).ok());
    compose_gate_key(input.as_deref(), lock_hash)
}

/// Record a compile-gate result. Writes one row per module in `modules`
/// **and** one batch row (no module) for the run — the batch row is the
/// whole-tree fact the gate actually established; the module rows are the
/// under-claiming projection onto the modules this run generated. Rows
/// replace by `(module, label)`. The lockfile is hashed here, i.e. after
/// the gate ran. Returns the rows written, batch row first.
pub fn record_gate(
    layout: &LayoutContext,
    modules: &[String],
    label: &str,
    pass: bool,
) -> Result<Vec<GateResult>> {
    if label.trim().is_empty() {
        bail!("gen-record-gate: --label must not be empty");
    }
    let lock = lockfile_hash(layout)?;
    let mut lineage = GenLineage::load(&layout.specdev_root)?;
    let mut written = Vec::with_capacity(modules.len() + 1);

    let batch = GateResult {
        module: None,
        label: label.to_string(),
        pass,
        key: compose_gate_key(None, &lock),
    };
    lineage.upsert_gate(batch.clone());
    written.push(batch);

    let mut names: Vec<&String> = modules.iter().collect();
    names.sort();
    names.dedup();
    for name in names {
        let row = GateResult {
            module: Some(name.clone()),
            label: label.to_string(),
            pass,
            key: current_gate_key(layout, Some(name), &lock),
        };
        lineage.upsert_gate(row.clone());
        written.push(row);
    }
    lineage.save(&layout.specdev_root)?;
    Ok(written)
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
    let mut report = GenDriftReport::default();
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
    /// No gen-lineage entry for the module — never generated. Regenerate.
    New,
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
}

#[derive(Debug, Clone, Serialize)]
pub struct GenPlanModule {
    pub module: String,
    pub category: GenPlanCategory,
    /// Second axis, independent of `category`: does a **passing** gate row
    /// exist for this module at the *current* key (fresh `input_hash` ⊕
    /// current lockfile hash)? `false` for a module that was never gated,
    /// failed its gate, or whose spec or lockfile moved since it passed.
    /// The skill's re-gate set is `category == unchanged && !verified`:
    /// run the compile gate again, do not regenerate.
    pub verified: bool,
    /// Affected output files — populated only for `edited` / `missing` so
    /// the skill can name them when it surfaces the module. Sorted; empty
    /// for the other categories.
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
    let module_names = crate::prompt::list_module_names(layout)?;
    let mut report = GenPlanReport::default();

    // The lockfile half of the gate key, computed once per plan. Read at
    // plan time it reflects the last gate that ran (or a `cargo update`
    // since) — exactly the thing a recorded pass must be compared against.
    let lock = lockfile_hash(layout)?;

    // Batch rows: whole-tree state per label.
    let batch_key = compose_gate_key(None, &lock);
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

    for name in &module_names {
        // Second axis: a passing gate row at the current key. Computed for
        // every category — a `new` or `stale` module is trivially
        // unverified (its key moved with its inputs), and saying so keeps
        // the field's meaning uniform.
        let current_key = current_gate_key(layout, Some(name), &lock);
        let verified = lineage
            .gates
            .iter()
            .any(|g| g.module.as_deref() == Some(name.as_str()) && g.pass && g.key == current_key);

        // Files recorded for this module (project-level entries excluded).
        let entries: Vec<(&String, &GenEntry)> = lineage
            .files
            .iter()
            .filter(|(_, e)| e.module.as_deref() == Some(name.as_str()))
            .collect();

        if entries.is_empty() {
            report.modules.push(GenPlanModule {
                module: name.clone(),
                category: GenPlanCategory::New,
                verified,
                files: Vec::new(),
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
            let current = crate::prompt::module_input_hash(layout, name).ok();
            let unchanged = uniform && first.is_some() && first == current;
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
            },
        );
        l.files.insert(
            "Cargo.toml".into(),
            GenEntry {
                module: None,
                generated_at: "2026-06-12T10:00:00Z".into(),
                content_hash: "sha256:def456".into(),
                input_hash: None,
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
            },
        );
        let text = l.to_toml_string().unwrap();
        assert!(text.contains("input_hash = \"sha256:in\""), "{text}");
    }
}
