//! Deterministic Cargo.toml emission from spec metadata.
//!
//! Reads `spec.toml` `[project]` + `architecture.toml` (`[platform]`,
//! `[runtime]`, `[logger]`, `[[external_deps]]`, `[msrv]`,
//! `[[features]]`) + module-level `external_deps[]`, and emits a
//! Cargo.toml whose content is a deterministic projection of the
//! spec.
//!
//! Every Cargo dep is in the spec; Cargo.toml is OUTPUT. This module
//! is the OUTPUT half.
//!
//! The analyzer doesn't generate Rust source code — but Cargo.toml is
//! configuration, not source. It's the same shape as `extract`
//! (deterministic projection in the analyzer; Claude not involved).
//! The skill's `spec-gen` flow calls this subcommand to emit
//! Cargo.toml; Claude emits the .rs source files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::layout::LayoutContext;
use crate::manifest::block_deps as bd_module;
use crate::manifest::{BinGate, BinSection, BlockDeps, PairedStub, Profile, ShapeKind};

/// One Cargo.toml `[dependencies]` entry, normalized.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CrateRef {
    #[serde(rename = "crate")]
    crate_name: String,
    /// Cargo version requirement. Optional ONLY when `source` is a local
    /// path dep — a path dep needs no registry version (F12-23), and the
    /// schema (`crate_reference.schema.yaml`) enforces that version is
    /// required for every other source. `None` renders a versionless dep.
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    source: Option<CrateSource>,
    #[serde(default)]
    features: Option<Vec<String>>,
    #[serde(default)]
    default_features: Option<bool>,
    /// Cargo's `optional = true` flag. Permitted alongside any source
    /// variant including `"workspace"` — Cargo accepts `optional` at
    /// the member-dep site even when the rest is workspace-inherited.
    #[serde(default)]
    optional: Option<bool>,
    /// When `true`, route this crate to `[dev-dependencies]` instead of
    /// `[dependencies]`. Test-only
    /// deps (test/bench/fuzz harness crates like `approx`, `proptest`,
    /// `criterion`) compile only for the test target. This is a routing
    /// flag, not a Cargo key — it never appears in the rendered value.
    #[serde(default)]
    test_only: Option<bool>,
}

/// Closed-set tagged union for `CrateReference.source`. Mirrors the
/// embedded schema (`src/embedded/crate_reference.schema.yaml`):
///   * `{ path = "..." }`            → path-dep
///   * `{ git = "...", rev/tag/branch = "..." }` → git-dep
///   * `"workspace"`                 → `<dep>.workspace = true`
///   * `"crates-io"`                 → explicit default registry
///
/// Anything else is rejected at schema-validate time. The renderer
/// dispatches on the variant — no silent drops (F-004).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
enum CrateSource {
    Tagged(CrateSourceTag),
    Path { path: String },
    Git {
        git: String,
        #[serde(default)]
        rev: Option<String>,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        branch: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
enum CrateSourceTag {
    #[serde(rename = "workspace")]
    Workspace,
    #[serde(rename = "crates-io")]
    CratesIo,
}

/// Result of a Cargo.toml render: the rendered text plus any
/// non-fatal warnings (e.g., version conflicts across modules).
///
/// `extra_files` carries paired source-tree side effects the analyzer
/// must produce alongside the Cargo.toml: the audio-plugin
/// `[[bin]]` row references `src/bin/standalone.rs`, so a write of the
/// rendered Cargo.toml without a paired stub at that path leaves cargo
/// in a broken state. Each entry is (project-relative path, body); the
/// caller writes the file only if the path doesn't already exist —
/// hand-edited bins are never overwritten.
#[derive(Debug, Clone)]
pub struct RenderedCargoToml {
    pub text: String,
    pub warnings: Vec<String>,
    pub extra_files: Vec<(PathBuf, String)>,
}

/// Render Cargo.toml for the project at `layout`. Dispatches across
/// four cases:
///
/// * **Workspace root** (`[workspace]` table set in spec.toml) →
///   [`render_workspace_root`]: emits `[workspace]` + `[workspace.package]`
///   + `[workspace.dependencies]`. Layout-driven.
/// * **Workspace member** (no `[workspace]` table, but an ancestor
///   spec.toml carries one whose `members[].path` references this
///   directory) → renders the per-shape body with workspace-aware
///   field inheritance: package fields owned by `[workspace_package]`
///   become `<field>.workspace = true`; deps with
///   `source = "workspace"` render as `{ workspace = true, ... }`.
/// * **Manifest-driven:** when the project's inlined
///   `specs/specdev/profile.yaml` exists, dispatch off
///   `cargo_toml.shape` (`library` / `binary` / `plugin` / `workspace`).
///   This is the preferred path — the analyzer holds zero compile-time
///   knowledge of profile names.
/// * **Backward-compat fallback:** for projects bootstrapped before the
///   manifest era, fall back to the legacy profile-name match. The
///   fallback is a transition aid removed in v1.1.
pub fn render(layout: &LayoutContext) -> Result<RenderedCargoToml> {
    if layout.spec.workspace.is_some() {
        return render_workspace_root(layout);
    }
    let workspace_ctx = WorkspaceContext::find_for(layout)?;

    // When `specs/specdev/profile.yaml` is present, dispatch off
    // `cargo_toml.shape`. The manifest body is loaded and
    // validated by `Profile::load`; failures (missing fields, unknown
    // shape, etc.) surface as MANIFEST-001 with line-precise context.
    if let Some(manifest) = load_inlined_profile(layout)? {
        return render_by_shape(layout, &manifest, workspace_ctx.as_ref());
    }

    // Backward-compat fallback for projects without a profile.yaml in
    // the inlined tree. Routes by `[project].profile` name to the same
    // per-shape render functions. Removed in v1.1.
    let profile = layout.spec.project.profile.as_str();
    match profile {
        "firmware-layered" => render_firmware(layout, workspace_ctx.as_ref()), // D51-fallback: pre-manifest backward-compat dispatch; remove in v1.1.
        "library-api" => render_library(layout, workspace_ctx.as_ref(), None), // D51-fallback: pre-manifest backward-compat dispatch; remove in v1.1.
        "audio-plugin" => render_audio_plugin(layout, workspace_ctx.as_ref(), None), // D51-fallback: pre-manifest backward-compat dispatch; remove in v1.1.
        "rust-workspace" => Err(anyhow!( // D51-fallback: pre-manifest backward-compat dispatch; remove in v1.1.
            "spec.toml at {} declares profile `rust-workspace` but has no \
             [workspace] table; add `[workspace]` with `[[workspace.members]]` \
             entries or change the profile",
            layout.spec_toml_path.display(),
        )),
        other => Err(anyhow!(
            "cargo-toml emission for profile `{other}` is not implemented; \
             supported: firmware-layered, library-api, audio-plugin, rust-workspace",
        )),
    }
}

/// Load `<specdev_root>/profile.yaml` if it exists. Returns `Ok(None)`
/// when the manifest is absent; `Err` only on a malformed/invalid
/// manifest. Bootstrap ensures every freshly-bootstrapped project has
/// this file when its profile ships one.
fn load_inlined_profile(layout: &LayoutContext) -> Result<Option<Profile>> {
    let path = layout.specdev_root.join("profile.yaml");
    if !path.is_file() {
        return Ok(None);
    }
    Profile::load(&path).map(Some)
}

/// Manifest-driven dispatch. Each `ShapeKind` routes to the
/// existing per-shape render function — pure routing, no
/// behavior change inside the renderers. Threads the manifest's
/// `cargo_toml.bin` section to `render_audio_plugin` so its `[[bin]]`
/// gate can be evaluated against the manifest spec instead of the
/// hardcoded `[plugin].formats` contains `"standalone"` literal.
fn render_by_shape(
    layout: &LayoutContext,
    manifest: &Profile,
    workspace_ctx: Option<&WorkspaceContext>,
) -> Result<RenderedCargoToml> {
    let cargo = manifest.cargo_toml.as_ref().ok_or_else(|| {
        anyhow!(
            "profile.yaml at {} has no `cargo_toml` section; cannot dispatch cargo-toml render",
            layout.specdev_root.join("profile.yaml").display(),
        )
    })?;
    match cargo.shape {
        ShapeKind::Library => render_library(layout, workspace_ctx, cargo.bin.as_ref()),
        ShapeKind::Binary => render_firmware(layout, workspace_ctx),
        ShapeKind::Plugin => render_audio_plugin(layout, workspace_ctx, cargo.bin.as_ref()),
        ShapeKind::Workspace => Err(anyhow!(
            "spec.toml at {} declares a profile with `cargo_toml.shape: workspace` \
             but has no [workspace] table; add `[workspace]` with \
             `[[workspace.members]]` entries or change the profile",
            layout.spec_toml_path.display(),
        )),
    }
}

/// Enumerate inlined blocks and collect each block's per-language
/// `deps.yaml` contribution as a flat `Vec<CrateRef>`. Blocks contribute
/// Cargo deps so users no longer mirror those crates in
/// `[[external_deps]]` by hand.
///
/// Discovery: scan `<inlined_blocks_dir>/*.schema.yaml` for block names
/// (one per file basename), then read
/// `<codegen_dir>/blocks/<block>/deps.yaml` via `BlockDeps::load`. The
/// load is `Ok(BlockDeps::default())` when the file is absent, so blocks
/// without a deps file (or blocks that simply ship no Rust deps) silently
/// contribute zero crates.
///
/// `recommended_external_deps[]` is intentionally NOT injected here —
/// it's surfaced by `/spec-design` as authoring suggestions;
/// auto-injection would erode the spec-is-source-of-truth invariant.
///
/// Order discipline: callers merge **after** architecture-level and
/// module-level deps so user-authored versions win on conflict (the
/// `merge_dep` semantics keep the first inserted entry; later
/// duplicates emit the conflict warning). Block contributions adding
/// new crates succeed; conflicts surface as warnings the spec author
/// resolves.
fn collect_inlined_block_deps(layout: &LayoutContext) -> Result<Vec<CrateRef>> {
    let mut out: Vec<CrateRef> = Vec::new();
    if !layout.inlined_blocks_dir.is_dir() {
        return Ok(out);
    }
    let mut block_names: Vec<String> = std::fs::read_dir(&layout.inlined_blocks_dir)
        .with_context(|| format!("reading {}", layout.inlined_blocks_dir.display()))?
        .filter_map(|r| r.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str())?;
            // Strip the `.schema.yaml` suffix to get the block name.
            // Skip non-block entries (defensive — directory shouldn't
            // hold anything else, but the inventory is unspecified).
            name.strip_suffix(".schema.yaml").map(String::from)
        })
        .collect();
    block_names.sort();
    for block in block_names {
        let deps_path = layout
            .codegen_dir
            .join("blocks")
            .join(&block)
            .join("deps.yaml");
        let deps = BlockDeps::load(&deps_path).with_context(|| {
            format!(
                "loading block deps.yaml for `{block}` at {}",
                deps_path.display()
            )
        })?;
        for entry in &deps.crates {
            out.push(block_crate_entry_to_crate_ref(entry));
        }
    }
    Ok(out)
}

/// Walk the inlined-bridge inventory and return every bridge's
/// `deps.yaml` contribution as a flat `Vec<CrateRef>`. Mirrors
/// [`collect_inlined_block_deps`] — same manifest schema, same loader,
/// path `<codegen_dir>/bridges/<bridge>/deps.yaml`. Bridges own
/// join-product crates: deps that exist only because two blocks are
/// paired (e.g. an async-flavored HAL crate), which neither block may
/// declare without reaching across the pairing.
///
/// Order discipline: callers merge after architecture-level,
/// module-level, AND block-level deps — user authoring wins first, then
/// blocks, then bridges (the most-derived contribution merges last).
fn collect_inlined_bridge_deps(layout: &LayoutContext) -> Result<Vec<CrateRef>> {
    let mut out: Vec<CrateRef> = Vec::new();
    if !layout.inlined_bridges_dir.is_dir() {
        return Ok(out);
    }
    let mut bridge_names: Vec<String> = std::fs::read_dir(&layout.inlined_bridges_dir)
        .with_context(|| format!("reading {}", layout.inlined_bridges_dir.display()))?
        .filter_map(|r| r.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str())?;
            name.strip_suffix(".schema.yaml").map(String::from)
        })
        .collect();
    bridge_names.sort();
    for bridge in bridge_names {
        let deps_path = layout
            .codegen_dir
            .join("bridges")
            .join(&bridge)
            .join("deps.yaml");
        let deps = BlockDeps::load(&deps_path).with_context(|| {
            format!(
                "loading bridge deps.yaml for `{bridge}` at {}",
                deps_path.display()
            )
        })?;
        for entry in &deps.crates {
            out.push(block_crate_entry_to_crate_ref(entry));
        }
    }
    Ok(out)
}

/// Bridge `BlockDeps::CrateEntry` (manifest shape) to `cargo_toml::CrateRef`
/// (renderer shape). The two structs carry the same fields under
/// different names; this fn is a transparent translation. The two
/// `CrateSource` enums are structurally identical — the duplication
/// exists because the manifest loader (validated against
/// `block_deps.schema.yaml`) and the renderer's parser (validated
/// against `crate_reference.schema.yaml`) ship as separate types. A
/// future v1.x cleanup could unify them; out of scope here.
fn block_crate_entry_to_crate_ref(entry: &bd_module::CrateEntry) -> CrateRef {
    CrateRef {
        crate_name: entry.name.clone(),
        version: Some(entry.version.clone()),
        source: entry.source.as_ref().map(translate_block_source),
        features: if entry.features.is_empty() {
            None
        } else {
            Some(entry.features.clone())
        },
        default_features: entry.default_features,
        optional: entry.optional,
        // Block/bridge deps are compile-time deps; test-only routing is a
        // module/architecture-level `external_deps` concern.
        test_only: None,
    }
}

fn translate_block_source(s: &bd_module::CrateSource) -> CrateSource {
    match s {
        bd_module::CrateSource::Tagged(bd_module::CrateSourceTag::Workspace) => {
            CrateSource::Tagged(CrateSourceTag::Workspace)
        }
        bd_module::CrateSource::Tagged(bd_module::CrateSourceTag::CratesIo) => {
            CrateSource::Tagged(CrateSourceTag::CratesIo)
        }
        bd_module::CrateSource::Path { path } => CrateSource::Path {
            path: path.clone(),
        },
        bd_module::CrateSource::Git {
            git,
            rev,
            tag,
            branch,
        } => CrateSource::Git {
            git: git.clone(),
            rev: rev.clone(),
            tag: tag.clone(),
            branch: branch.clone(),
        },
    }
}

/// Evaluate a closed-enum `BinGate` against architecture.toml. Returns
/// `true` when the gate condition is met (i.e. the bin should emit).
/// Unknown gate variants are deliberately a compile error — the closed
/// enum forces an analyzer-side dispatch addition for any new gate,
/// mirroring the discipline applied to ShapeKind.
fn evaluate_bin_gate(gate: &BinGate, arch: &toml::value::Table) -> bool {
    match gate {
        BinGate::ArchitectureArrayContains { table, field, value } => arch
            .get(table.as_str())
            .and_then(toml::Value::as_table)
            .and_then(|t| t.get(field.as_str()))
            .and_then(toml::Value::as_array)
            .map(|arr| arr.iter().any(|v| v.as_str() == Some(value.as_str())))
            .unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// Workspace ancestor detection
// ---------------------------------------------------------------------------

/// Workspace context carried alongside a member-mode render. Holds the
/// workspace's parsed `[workspace_package]` (which member fields can
/// inherit) and its hoisted `[[workspace_dependencies]]` (which member
/// deps can reference via `source = "workspace"`).
#[derive(Debug, Clone)]
pub(crate) struct WorkspaceContext {
    /// Set of `[workspace_package]` field names present (e.g.
    /// `"version"`, `"edition"`, `"rust-version"`, `"license"`,
    /// `"repository"`). Members emit `<field>.workspace = true` for any
    /// field in this set; absent fields stay member-literal.
    pub package_fields: BTreeSet<String>,
    /// Map from hoisted dep name → CrateRef. Members may reference
    /// these via `source = "workspace"`; the v1 emitter trusts the
    /// member's spec (no equivalence check against this map). The map
    /// is reserved for the future reconciliation pass.
    #[allow(dead_code)]
    pub workspace_deps: BTreeMap<String, CrateRef>,
}

impl WorkspaceContext {
    /// Walk up from the member's spec.toml to find an ancestor spec.toml
    /// carrying `[workspace]` whose `members[].path` references the
    /// member's directory (relative to the workspace root). Returns
    /// `Ok(None)` if no workspace ancestor is found (standalone project).
    fn find_for(layout: &LayoutContext) -> Result<Option<Self>> {
        let member_dir = layout.project_root.as_path();
        // Start one directory above the member: a workspace's spec.toml
        // is by definition outside the member's tree.
        let mut cursor = match member_dir.parent() {
            Some(p) => p,
            None => return Ok(None),
        };
        loop {
            let candidate = cursor.join("spec.toml");
            if candidate.is_file() {
                if let Some(ctx) = Self::try_load(&candidate, member_dir)? {
                    return Ok(Some(ctx));
                }
            }
            match cursor.parent() {
                Some(p) => cursor = p,
                None => return Ok(None),
            }
        }
    }

    fn try_load(
        candidate_spec: &Path,
        member_dir: &Path,
    ) -> Result<Option<Self>> {
        // Parse the candidate; if it's not a workspace spec or doesn't
        // include this member, return None and let the caller keep
        // walking. Parse errors short-circuit to None — a malformed
        // ancestor spec.toml is the analyzer's job to flag during its
        // own validate run, not cargo-toml's.
        let text = std::fs::read_to_string(candidate_spec)
            .with_context(|| format!("reading {}", candidate_spec.display()))?;
        let parsed: serde_json::Value = match toml::from_str::<serde_json::Value>(&text) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };
        let workspace = match parsed.get("workspace") {
            Some(w) if w.is_object() => w,
            _ => return Ok(None),
        };
        let candidate_root = candidate_spec.parent().unwrap_or(candidate_spec);
        let rel_member = match member_dir.strip_prefix(candidate_root) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        // Normalize to forward slashes for comparison against the
        // members[].path field, which is always written in spec-toml
        // forward-slash form.
        let rel_member_str = rel_member.to_string_lossy().replace('\\', "/");
        let members = workspace
            .get("members")
            .and_then(|v| v.as_array())
            .map(|arr| arr.as_slice())
            .unwrap_or(&[]);
        let included = members.iter().any(|m| {
            m.get("path")
                .and_then(|p| p.as_str())
                .map(|p| {
                    // Trim trailing slash on stored paths for tolerance.
                    p.trim_end_matches('/') == rel_member_str
                })
                .unwrap_or(false)
        });
        if !included {
            return Ok(None);
        }

        // Found the workspace ancestor. Load architecture.toml from the
        // workspace's `<paths.specs>/architecture.toml`. Default to
        // "specs/architecture.toml" if [paths] is unparseable.
        let specs_dir = parsed
            .get("paths")
            .and_then(|p| p.get("specs"))
            .and_then(|s| s.as_str())
            .unwrap_or("specs");
        let arch_path = candidate_root.join(specs_dir).join("architecture.toml");
        let arch = read_architecture(&arch_path)?;

        let mut package_fields: BTreeSet<String> = BTreeSet::new();
        if let Some(wp) = arch.get("workspace_package").and_then(toml::Value::as_table) {
            for (k, _) in wp.iter() {
                package_fields.insert(k.clone());
            }
        }

        let mut workspace_deps: BTreeMap<String, CrateRef> = BTreeMap::new();
        for c in extract_crate_ref_array(arch.get("workspace_dependencies"))? {
            workspace_deps.insert(c.crate_name.clone(), c);
        }

        Ok(Some(WorkspaceContext {
            package_fields,
            workspace_deps,
        }))
    }
}

// ---------------------------------------------------------------------------
// firmware-layered
// ---------------------------------------------------------------------------

fn render_firmware(
    layout: &LayoutContext,
    workspace: Option<&WorkspaceContext>,
) -> Result<RenderedCargoToml> {
    let project = &layout.spec.project;
    let arch = read_architecture(&layout.architecture_toml_path)?;

    let mut deps: BTreeMap<String, CrateRef> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // [platform.mcu_core] / [platform.mcu_runtime]
    if let Some(platform) = arch.get("platform").and_then(toml::Value::as_table) {
        for key in ["mcu_core", "mcu_runtime"] {
            if let Some(c) = parse_crate_ref(platform.get(key))? {
                merge_dep(&mut deps, c, &mut warnings);
            }
        }
    }
    // [runtime.panic_handler] / [runtime.allocator] / [runtime.critical_section_impl]
    if let Some(runtime) = arch.get("runtime").and_then(toml::Value::as_table) {
        for key in ["panic_handler", "allocator", "critical_section_impl"] {
            if let Some(c) = parse_crate_ref(runtime.get(key))? {
                merge_dep(&mut deps, c, &mut warnings);
            }
        }
    }
    // [logger.provider] / [logger.backend]
    if let Some(logger) = arch.get("logger").and_then(toml::Value::as_table) {
        for key in ["provider", "backend"] {
            if let Some(c) = parse_crate_ref(logger.get(key))? {
                merge_dep(&mut deps, c, &mut warnings);
            }
        }
    }
    // [[external_deps]]
    for c in extract_crate_ref_array(arch.get("external_deps"))? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Module-level external_deps[] across every module file.
    for c in collect_module_external_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Block-contributed deps via `<codegen>/blocks/<block>/deps.yaml`.
    // Merged AFTER user-authored deps so user spec wins on conflict.
    for c in collect_inlined_block_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Bridge-contributed join-product deps. Merged AFTER block deps
    // (user > blocks > bridges; first insert wins, conflicts warn).
    for c in collect_inlined_bridge_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }

    let mut text = String::new();
    write_package_section(&mut text, project, /* rust_version */ None, workspace);
    text.push('\n');

    // Crate target. A binary-shaped profile (firmware-layered) renders
    // `[[bin]]` by default; a per-package `crate_shape = "library"` override
    // (F12-22) renders `[lib]` instead — for a driver/HAL-shim library member
    // of a firmware workspace. Dependencies are identical either way.
    match project.crate_shape {
        Some(crate::config::CrateShape::Library) => {
            text.push_str("[lib]\n");
            text.push_str(&format!("name = {}\n", quote_str(&crate_ident(&project.name))));
            text.push_str("path = \"src/lib.rs\"\n");
        }
        _ => {
            text.push_str("[[bin]]\n");
            text.push_str(&format!("name = {}\n", quote_str(&project.name)));
            text.push_str("path = \"src/main.rs\"\n");
        }
    }
    text.push('\n');

    write_dependencies_section(&mut text, &deps);

    Ok(RenderedCargoToml { text, warnings, extra_files: Vec::new() })
}

// ---------------------------------------------------------------------------
// library-api
// ---------------------------------------------------------------------------

fn render_library(
    layout: &LayoutContext,
    workspace: Option<&WorkspaceContext>,
    manifest_bin: Option<&BinSection>,
) -> Result<RenderedCargoToml> {
    let project = &layout.spec.project;
    let arch = read_architecture(&layout.architecture_toml_path)?;

    let msrv = arch
        .get("msrv")
        .and_then(toml::Value::as_str)
        .map(String::from);

    let parsed_features = parse_library_features(arch.get("features"))?;

    let mut deps: BTreeMap<String, CrateRef> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // Library architecture currently has no [[external_deps]]
    // (library-api's typed picks live at module level), but accept it
    // if present for future flexibility.
    for c in extract_crate_ref_array(arch.get("external_deps"))? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    for c in collect_module_external_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Block-contributed deps. Merged AFTER user-authored deps
    // so user spec wins on conflict.
    for c in collect_inlined_block_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Bridge-contributed join-product deps. Merged AFTER block deps
    // (user > blocks > bridges; first insert wins, conflicts warn).
    for c in collect_inlined_bridge_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }

    let mut text = String::new();
    // Member-mode skips literal rust-version when [workspace_package]
    // owns it (will be emitted as `rust-version.workspace = true` by
    // write_package_section below); otherwise pass through architecture
    // msrv as today.
    let rust_version = match workspace {
        Some(w) if w.package_fields.contains("rust-version") => None,
        _ => msrv.as_deref(),
    };
    write_package_section(&mut text, project, rust_version, workspace);
    text.push('\n');

    text.push_str("[lib]\n\n");

    // Optional `[[bin]]` alongside the `[lib]`. A library-shaped profile
    // that also ships an executable (the `src/lib.rs` + `src/main.rs`
    // crate — a testable core plus a thin entry point) declares
    // `cargo_toml.bin` in its manifest; profiles that don't, don't.
    // Same gate semantics as the plugin shape, but name/path come from
    // the manifest templates rather than a hardcoded pair.
    let emit_bin = match manifest_bin {
        Some(b) => match b.gate.as_ref() {
            Some(gate) => evaluate_bin_gate(gate, &arch),
            None => true,
        },
        None => false,
    };
    if let (true, Some(b)) = (emit_bin, manifest_bin) {
        text.push_str("[[bin]]\n");
        text.push_str(&format!(
            "name = {}\n",
            quote_str(&b.name_template.replace("{project_name}", &project.name))
        ));
        text.push_str(&format!(
            "path = {}\n",
            quote_str(&b.path_template.replace("{project_name}", &project.name))
        ));
        text.push('\n');
    }

    if !parsed_features.features.is_empty() {
        text.push_str("[features]\n");
        // Synthetic `default = [list]` line — emitted always when there's
        // a [features] section, so member crates that have features but
        // no defaults still render `default = []` (matches Cargo
        // convention and the zfm dogfood baseline).
        let default_arr = parsed_features
            .defaults
            .iter()
            .map(|d| quote_str(d))
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!("default = [{}]\n", default_arr));
        for (name, deps) in &parsed_features.features {
            if name == "default" {
                // User-authored `default` feature would collide with the
                // synthetic line; skip and emit the literal at synthesis
                // time instead. (Real projects don't author `default`
                // explicitly; if they do, the synthesis above already
                // covers it via the `default = true` flag.)
                continue;
            }
            let arr = deps
                .iter()
                .map(|d| quote_str(d))
                .collect::<Vec<_>>()
                .join(", ");
            text.push_str(&format!("{} = [{}]\n", name, arr));
        }
        text.push('\n');
    }

    write_dependencies_section(&mut text, &deps);

    // Pair an emitted `[[bin]]` with a stub source file so the crate
    // compiles on a fresh emit (the caller skips the write when the file
    // already exists, so a hand-authored entry point is never
    // overwritten). Mirrors the plugin shape's standalone pairing.
    let mut extra_files: Vec<(PathBuf, String)> = Vec::new();
    if let (true, Some(stub), Some(b)) = (
        emit_bin,
        manifest_bin.and_then(|b| b.paired_stub.as_ref()),
        manifest_bin,
    ) {
        let body = read_paired_stub_body(layout, stub, &project.name)?;
        extra_files.push((
            PathBuf::from(b.path_template.replace("{project_name}", &project.name)),
            body,
        ));
    }

    Ok(RenderedCargoToml { text, warnings, extra_files })
}

// ---------------------------------------------------------------------------
// audio-plugin
// ---------------------------------------------------------------------------

/// Render Cargo.toml for an `audio-plugin` profile project.
///
/// The plugin crate is always a `cdylib` (the C-ABI surface VST3 and
/// CLAP both require). When
/// `[plugin].formats` contains `"standalone"`, also emit a `[[bin]]`
/// target `name = "<project-name>-standalone"`, `path =
/// "src/bin/standalone.rs"` — the bin links the same lib so the
/// `Plugin` impl is reused.
///
/// Architecture-level deps come from `[runtime].panic_handler`-style
/// concrete crates is out of scope for v1 (the panic_handler is a
/// closed-enum strategy, not a CrateReference). Plugin-side cargo
/// deps live entirely in `[[external_deps]]` (architecture-wide) +
/// per-module `external_deps[]`.
///
/// `[[features]]` parity with library-api is honored: when the
/// architecture declares features, emit a `[features]` section with
/// the synthetic `default = [...]` line.
fn render_audio_plugin(
    layout: &LayoutContext,
    workspace: Option<&WorkspaceContext>,
    manifest_bin: Option<&BinSection>,
) -> Result<RenderedCargoToml> {
    let project = &layout.spec.project;
    let arch = read_architecture(&layout.architecture_toml_path)?;

    let parsed_features = parse_library_features(arch.get("features"))?;

    // Bin emission gate. When the manifest carries a
    // `cargo_toml.bin.gate`, evaluate it (closed-set `BinGate` enum).
    // When the manifest is absent or carries no
    // gate, fall back to the legacy hardcoded `[plugin].formats`-
    // contains-`"standalone"` check. Both paths produce the same boolean
    // for any architecture.toml the audio-plugin profile authors today.
    let emit_standalone_bin = match manifest_bin.and_then(|b| b.gate.as_ref()) {
        Some(gate) => evaluate_bin_gate(gate, &arch),
        None => {
            let plugin_table = arch.get("plugin").and_then(toml::Value::as_table);
            let formats: Vec<String> = plugin_table
                .and_then(|p| p.get("formats"))
                .and_then(toml::Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            formats.iter().any(|f| f == "standalone")
        }
    };

    let mut deps: BTreeMap<String, CrateRef> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // Architecture-level [[external_deps]] (unstructured open list).
    for c in extract_crate_ref_array(arch.get("external_deps"))? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Per-module external_deps[].
    for c in collect_module_external_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Block-contributed deps. Merged AFTER user-authored deps
    // so user spec wins on conflict.
    for c in collect_inlined_block_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    // Bridge-contributed join-product deps. Merged AFTER block deps
    // (user > blocks > bridges; first insert wins, conflicts warn).
    for c in collect_inlined_bridge_deps(layout)? {
        merge_dep(&mut deps, c, &mut warnings);
    }

    let mut text = String::new();
    write_package_section(&mut text, project, /* rust_version */ None, workspace);
    text.push('\n');

    // [lib] always cdylib for audio-plugin. Plugin shells live in
    // src/lib.rs and run nih_export_*! macros that target the C ABI.
    text.push_str("[lib]\n");
    text.push_str("crate-type = [\"cdylib\"]\n");
    text.push('\n');

    // [[bin]] for the standalone format. Name is derived from
    // project.name (the crate name) — display-name from
    // [plugin].name may contain spaces / punctuation cargo rejects.
    if emit_standalone_bin {
        text.push_str("[[bin]]\n");
        text.push_str(&format!(
            "name = {}\n",
            quote_str(&format!("{}-standalone", project.name))
        ));
        text.push_str("path = \"src/bin/standalone.rs\"\n");
        text.push('\n');
    }

    // [features] — same synthesis as library-api (always-emit `default
    // = [...]` line when [features] is present).
    if !parsed_features.features.is_empty() {
        text.push_str("[features]\n");
        let default_arr = parsed_features
            .defaults
            .iter()
            .map(|d| quote_str(d))
            .collect::<Vec<_>>()
            .join(", ");
        text.push_str(&format!("default = [{}]\n", default_arr));
        for (name, feature_deps) in &parsed_features.features {
            if name == "default" {
                continue;
            }
            let arr = feature_deps
                .iter()
                .map(|d| quote_str(d))
                .collect::<Vec<_>>()
                .join(", ");
            text.push_str(&format!("{} = [{}]\n", name, arr));
        }
        text.push('\n');
    }

    write_dependencies_section(&mut text, &deps);

    // Pair the [[bin]] standalone row with a stub source file so
    // the project compiles immediately after a fresh emit. The caller
    // skips the write if the file already exists, so hand-edited bins
    // are never overwritten.
    //
    // When the manifest path provides a `paired_stub.template_file`
    // entry, read the file from `<specdev_root>/<template_file>` (copied
    // there at bootstrap time) and substitute `{project_name}` with
    // `[project].name`. When the manifest path is absent
    // (backward-compat), fall back to the legacy
    // `standalone_bin_stub()` literal — preserves byte-parity for projects
    // that pre-date the manifest era.
    let mut extra_files: Vec<(PathBuf, String)> = Vec::new();
    if emit_standalone_bin {
        let body = match manifest_bin.and_then(|b| b.paired_stub.as_ref()) {
            Some(stub) => read_paired_stub_body(layout, stub, &project.name)?,
            None => standalone_bin_stub(&project.name),
        };
        extra_files.push((PathBuf::from("src/bin/standalone.rs"), body));
    }

    Ok(RenderedCargoToml {
        text,
        warnings,
        extra_files,
    })
}

/// Read the paired-stub template referenced by the audio-plugin
/// manifest's `cargo_toml.bin.paired_stub.template_file` and substitute
/// `{project_name}` with the project's crate name. The template path is
/// stored in the manifest as relative to the profile dir (e.g.
/// `templates/standalone-stub.rs.template`); after bootstrap copies the
/// `templates/` tree to `<specdev_root>/templates/`, the same relative
/// path resolves under the inlined tree. The only allowed token in this
/// body is `{project_name}`; other `{...}` patterns pass through
/// unchanged (closed-grammar enforcement of the body itself is a v1.x
/// concern — the template is plugin-author content and a stray brace is
/// the author's bug to find).
fn read_paired_stub_body(
    layout: &LayoutContext,
    paired_stub: &PairedStub,
    project_name: &str,
) -> Result<String> {
    let template_path = layout.specdev_root.join(&paired_stub.template_file);
    let body = std::fs::read_to_string(&template_path).with_context(|| {
        format!(
            "reading paired_stub template at {} (referenced from profile.yaml)",
            template_path.display()
        )
    })?;
    Ok(body.replace("{project_name}", project_name))
}

/// Minimal `src/bin/standalone.rs` stub paired with the
/// `[[bin]]` row when `[plugin].formats` includes `"standalone"`. The
/// body uses `nih_plug::prelude::nih_export_standalone::<MyPlugin>()`
/// — author replaces `MyPlugin` with their concrete plugin type once
/// it lands. Until then, the stub references a placeholder name behind
/// a `compile_error!` so a fresh emit produces a friendly compile-time
/// reminder instead of a confusing missing-file error.
///
/// Retained as the **backward-compat fallback** for projects without a
/// manifest (no `paired_stub.template_file`). The manifest path reads
/// from the inlined template instead — see `read_paired_stub_body`.
fn standalone_bin_stub(crate_name: &str) -> String {
    format!(
        "//! Standalone host for `{crate_name}` (F-016 stub).\n\
         //!\n\
         //! Generated by specdev-analyzer alongside the `[[bin]]` row in\n\
         //! Cargo.toml. Replace `MyPlugin` below with the concrete plugin\n\
         //! type your `lib.rs` exports, then delete this notice.\n\n\
         compile_error!(\n    \"replace `MyPlugin` in src/bin/standalone.rs with the plugin type \\\n     your lib.rs declares, then delete this compile_error! line\"\n);\n\n\
         // fn main() {{\n\
         //     nih_plug::prelude::nih_export_standalone::<MyPlugin>();\n\
         // }}\n",
    )
}

// ---------------------------------------------------------------------------
// rust-workspace
// ---------------------------------------------------------------------------

/// Render the workspace-root `Cargo.toml` for a `rust-workspace` project.
/// Emits `[workspace]` (resolver + members), `[workspace.package]`
/// (canonical fields members inherit), and `[workspace.dependencies]`
/// (hoisted candidates members reference via `source = "workspace"`).
///
/// Members are written in `[[workspace.members]]` declaration order;
/// dependencies are lex-sorted (matching the per-member
/// `[dependencies]` convention). No timestamps, no machine paths —
/// byte-stable across runs.
fn render_workspace_root(layout: &LayoutContext) -> Result<RenderedCargoToml> {
    let workspace_section = layout
        .spec
        .workspace
        .as_ref()
        .expect("render_workspace_root only called when [workspace] is set");

    let arch = read_architecture(&layout.architecture_toml_path)?;
    let warnings: Vec<String> = Vec::new();

    let mut text = String::new();

    // [workspace] block: resolver + members list.
    text.push_str("[workspace]\n");
    let resolver = workspace_section
        .resolver
        .as_deref()
        .unwrap_or("2");
    text.push_str(&format!("resolver = {}\n", quote_str(resolver)));
    text.push_str("members = [\n");
    for member in &workspace_section.members {
        text.push_str(&format!("    {},\n", quote_str(&member.path)));
    }
    text.push_str("]\n");

    // [workspace.package] — only the fields actually present in
    // architecture.toml's [workspace_package]. Emit in Cargo's
    // idiomatic order (matches zfm dogfood baseline): identity →
    // license → repository → toolchain version.
    if let Some(wp) = arch.get("workspace_package").and_then(toml::Value::as_table) {
        if !wp.is_empty() {
            text.push('\n');
            text.push_str("[workspace.package]\n");
            for key in ["version", "edition", "license", "repository", "rust-version"] {
                if let Some(v) = wp.get(key).and_then(toml::Value::as_str) {
                    text.push_str(&format!("{} = {}\n", key, quote_str(v)));
                }
            }
        }
    }

    // [workspace.dependencies] — declare-then-emit (lex-sorted by
    // crate name). Each entry uses the same render path as
    // per-member [dependencies].
    let mut workspace_deps: BTreeMap<String, CrateRef> = BTreeMap::new();
    for c in extract_crate_ref_array(arch.get("workspace_dependencies"))? {
        workspace_deps.insert(c.crate_name.clone(), c);
    }
    if !workspace_deps.is_empty() {
        text.push('\n');
        text.push_str("[workspace.dependencies]\n");
        for (name, c) in &workspace_deps {
            text.push_str(&format!("{} = {}\n", name, render_dep_value(c)));
        }
    }

    Ok(RenderedCargoToml { text, warnings, extra_files: Vec::new() })
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn read_architecture(path: &Path) -> Result<toml::value::Table> {
    if !path.is_file() {
        return Ok(toml::value::Table::new());
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let value: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    match value {
        toml::Value::Table(t) => Ok(t),
        _ => Err(anyhow!(
            "{} did not parse as a TOML table",
            path.display()
        )),
    }
}

fn parse_crate_ref(value: Option<&toml::Value>) -> Result<Option<CrateRef>> {
    let Some(v) = value else { return Ok(None) };
    let json = serde_json::to_value(v)?;
    let parsed: CrateRef = serde_json::from_value(json)
        .map_err(|e| anyhow!("CrateReference parse error: {e}"))?;
    Ok(Some(parsed))
}

fn extract_crate_ref_array(value: Option<&toml::Value>) -> Result<Vec<CrateRef>> {
    let Some(v) = value else { return Ok(Vec::new()) };
    let arr = match v.as_array() {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for item in arr {
        if let Some(c) = parse_crate_ref(Some(item))? {
            out.push(c);
        }
    }
    Ok(out)
}

fn collect_module_external_deps(layout: &LayoutContext) -> Result<Vec<CrateRef>> {
    let mut out: Vec<CrateRef> = Vec::new();
    if !layout.modules_dir.is_dir() {
        return Ok(out);
    }
    let mut paths: Vec<_> = std::fs::read_dir(&layout.modules_dir)
        .with_context(|| format!("reading {}", layout.modules_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().map(|s| s == "toml").unwrap_or(false))
        .collect();
    paths.sort();
    for path in &paths {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let value: toml::Value = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        for c in extract_crate_ref_array(value.get("external_deps"))? {
            out.push(c);
        }
    }
    Ok(out)
}

/// Insert `c` into `deps`. If a different version of the same crate
/// already exists, push a warning and keep the existing entry
/// (deterministic: lex-first source wins; warning surfaces the
/// conflict so the spec author can reconcile).
fn merge_dep(deps: &mut BTreeMap<String, CrateRef>, c: CrateRef, warnings: &mut Vec<String>) {
    match deps.get(&c.crate_name) {
        None => {
            deps.insert(c.crate_name.clone(), c);
        }
        Some(existing) if existing == &c => {
            // exact duplicate; ignore.
        }
        Some(existing) if existing.version != c.version => {
            let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "<none>".to_string());
            warnings.push(format!(
                "crate `{}` declared with conflicting versions ({} vs {}); using {}",
                c.crate_name,
                show(&existing.version),
                show(&c.version),
                show(&existing.version),
            ));
        }
        Some(existing) => {
            // Same version, different ancillary fields (features /
            // default_features / source). Union features; first source
            // wins; warn if default_features disagrees.
            let mut merged = existing.clone();
            let combined: BTreeSet<String> = existing
                .features
                .clone()
                .unwrap_or_default()
                .into_iter()
                .chain(c.features.clone().unwrap_or_default())
                .collect();
            merged.features = if combined.is_empty() {
                None
            } else {
                Some(combined.into_iter().collect())
            };
            if existing.default_features.is_some()
                && c.default_features.is_some()
                && existing.default_features != c.default_features
            {
                warnings.push(format!(
                    "crate `{}` declared with conflicting `default_features`; using {:?}",
                    c.crate_name, existing.default_features,
                ));
            } else if merged.default_features.is_none() {
                merged.default_features = c.default_features;
            }
            deps.insert(c.crate_name.clone(), merged);
        }
    }
}

/// Parsed library-api `[features]` table. Each entry carries the feature's
/// merged-and-sorted dep list (`depends_on` ∪ `cargo_deps`) plus the
/// `default` flag used to synthesize the synthetic `default = [...]`
/// line at emit time.
#[derive(Debug, Clone, Default)]
pub(crate) struct ParsedFeatures {
    /// Per-feature dep list, lex-sorted within each entry. Keyed by
    /// feature name; outer ordering is BTreeMap-stable for emission.
    pub features: BTreeMap<String, Vec<String>>,
    /// Names of features marked `default = true`. Lex-sorted; emitted
    /// as the synthetic `default = [...]` line.
    pub defaults: Vec<String>,
}

fn parse_library_features(value: Option<&toml::Value>) -> Result<ParsedFeatures> {
    let mut out = ParsedFeatures::default();
    let Some(arr) = value.and_then(toml::Value::as_array) else {
        return Ok(out);
    };
    for item in arr {
        let Some(t) = item.as_table() else { continue };
        let Some(name) = t.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let depends_on: Vec<String> = t
            .get("depends_on")
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let cargo_deps: Vec<String> = t
            .get("cargo_deps")
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut deps: Vec<String> =
            depends_on.into_iter().chain(cargo_deps.into_iter()).collect();
        deps.sort();
        deps.dedup();
        out.features.insert(name.to_string(), deps);

        if t.get("default").and_then(toml::Value::as_bool).unwrap_or(false) {
            out.defaults.push(name.to_string());
        }
    }
    out.defaults.sort();
    out.defaults.dedup();
    Ok(out)
}

/// Write the `[package]` section. When `workspace` is `Some`, fields
/// owned by the workspace's `[workspace_package]` table are emitted as
/// `<field>.workspace = true` instead of literal copies; the five
/// inheritable fields per G-005 are: `version`, `edition`,
/// `rust-version`, `license`, `repository`. `name` and `description`
/// are always per-crate (never workspace-inherited).
///
/// Field emission order matches Cargo's idiomatic [package] ordering
/// and the zfm dogfood baseline byte-for-byte: name → version → edition
/// → license → repository → rust-version → description.
fn write_package_section(
    text: &mut String,
    project: &crate::config::ProjectSection,
    rust_version: Option<&str>,
    workspace: Option<&WorkspaceContext>,
) {
    text.push_str("[package]\n");
    text.push_str(&format!("name = {}\n", quote_str(&project.name)));

    let inherits = |field: &str| -> bool {
        workspace
            .map(|w| w.package_fields.contains(field))
            .unwrap_or(false)
    };

    // version — workspace-inheritable; not on standalone ProjectSection
    // today (could become a per-crate override field later).
    if inherits("version") {
        text.push_str("version.workspace = true\n");
    }

    // edition — defaults to "2021" when unset; workspace-inheritable.
    if inherits("edition") {
        text.push_str("edition.workspace = true\n");
    } else {
        let edition = project.edition.as_deref().unwrap_or("2021");
        text.push_str(&format!("edition = {}\n", quote_str(edition)));
    }

    // license — workspace-inheritable.
    if inherits("license") {
        text.push_str("license.workspace = true\n");
    } else if let Some(license) = &project.license {
        text.push_str(&format!("license = {}\n", quote_str(license)));
    }

    // repository — workspace-inheritable; member's own
    // [project].repository overrides if explicitly set.
    if let Some(repo) = &project.repository {
        text.push_str(&format!("repository = {}\n", quote_str(repo)));
    } else if inherits("repository") {
        text.push_str("repository.workspace = true\n");
    }

    // rust-version — workspace-inheritable; otherwise pass-through
    // from caller (firmware: always None; library: from architecture msrv).
    if inherits("rust-version") {
        text.push_str("rust-version.workspace = true\n");
    } else if let Some(rv) = rust_version {
        text.push_str(&format!("rust-version = {}\n", quote_str(rv)));
    }

    // description — always per-crate.
    if let Some(desc) = &project.description {
        text.push_str(&format!("description = {}\n", quote_str(desc)));
    }
}

fn write_dependencies_section(text: &mut String, deps: &BTreeMap<String, CrateRef>) {
    // Partition by the `test_only` routing flag: regular deps go under
    // `[dependencies]`, test-only deps under `[dev-dependencies]`.
    // `test_only` is never rendered into the value.
    text.push_str("[dependencies]\n");
    for (name, c) in deps.iter().filter(|(_, c)| c.test_only != Some(true)) {
        text.push_str(&format!("{} = {}\n", name, render_dep_value(c)));
    }
    let dev: Vec<(&String, &CrateRef)> = deps
        .iter()
        .filter(|(_, c)| c.test_only == Some(true))
        .collect();
    if !dev.is_empty() {
        text.push_str("\n[dev-dependencies]\n");
        for (name, c) in dev {
            text.push_str(&format!("{} = {}\n", name, render_dep_value(c)));
        }
    }
}

/// Render a CrateRef as a Cargo.toml inline value.
///
/// Bare `version = "x"` form when the crate has no extras; inline-
/// table form (`{ version = "x", features = [...], ... }`) otherwise.
/// Matches Cargo's idiomatic style and minimizes diff churn when
/// extras are added/removed.
///
/// Source dispatch (F-004):
///   * `CrateSourceTag::Workspace` → emits `{ workspace = true }`
///     and OMITS version/features/default-features (Cargo's workspace
///     inheritance forbids those at the member-dep site).
///   * `CrateSourceTag::CratesIo`  → omitted entirely (default registry).
///   * `CrateSource::Path { path }` → adds `path = "..."`.
///   * `CrateSource::Git { git, rev/tag/branch }` → adds `git = "..."`
///     plus whichever of rev/tag/branch is set.
fn render_dep_value(c: &CrateRef) -> String {
    // Workspace-inherited dep: `{ workspace = true [, features = ...]
    // [, optional = true] }`. Cargo forbids version / default-features /
    // path / git at the member site when source is workspace, but allows
    // additive `features` and per-member `optional`.
    if let Some(CrateSource::Tagged(CrateSourceTag::Workspace)) = c.source {
        let mut parts: Vec<String> = vec!["workspace = true".to_string()];
        if let Some(features) = &c.features {
            if !features.is_empty() {
                let mut sorted = features.clone();
                sorted.sort();
                sorted.dedup();
                let arr = sorted
                    .iter()
                    .map(|f| quote_str(f))
                    .collect::<Vec<_>>()
                    .join(", ");
                parts.push(format!("features = [{}]", arr));
            }
        }
        if c.optional == Some(true) {
            parts.push("optional = true".to_string());
        }
        return format!("{{ {} }}", parts.join(", "));
    }

    let source_parts = match &c.source {
        None | Some(CrateSource::Tagged(CrateSourceTag::CratesIo)) => Vec::new(),
        Some(CrateSource::Path { path }) => {
            vec![format!("path = {}", quote_str(path))]
        }
        Some(CrateSource::Git { git, rev, tag, branch }) => {
            let mut parts = vec![format!("git = {}", quote_str(git))];
            if let Some(r) = rev {
                parts.push(format!("rev = {}", quote_str(r)));
            }
            if let Some(t) = tag {
                parts.push(format!("tag = {}", quote_str(t)));
            }
            if let Some(b) = branch {
                parts.push(format!("branch = {}", quote_str(b)));
            }
            parts
        }
        Some(CrateSource::Tagged(CrateSourceTag::Workspace)) => unreachable!(),
    };

    let has_extras = !source_parts.is_empty()
        || c.features.as_ref().map(|f| !f.is_empty()).unwrap_or(false)
        || c.default_features.is_some()
        || c.optional == Some(true);
    // Bare `version = "x"` form only when there are no extras AND a version
    // is present. A versionless path dep always carries a `path` extra, so it
    // never reaches the bare form; it renders as `{ path = "..." }`.
    if !has_extras {
        if let Some(v) = &c.version {
            return quote_str(v);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(v) = &c.version {
        parts.push(format!("version = {}", quote_str(v)));
    }
    if let Some(features) = &c.features {
        if !features.is_empty() {
            let mut sorted = features.clone();
            sorted.sort();
            sorted.dedup();
            let arr = sorted
                .iter()
                .map(|f| quote_str(f))
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("features = [{}]", arr));
        }
    }
    if let Some(df) = c.default_features {
        parts.push(format!("default-features = {}", df));
    }
    if c.optional == Some(true) {
        parts.push("optional = true".to_string());
    }
    parts.extend(source_parts);
    format!("{{ {} }}", parts.join(", "))
}

/// A crate package name (`sensor-driver`) as a Rust crate identifier
/// (`sensor_driver`) — Cargo `[lib] name` and `use` paths require the
/// hyphen→underscore form.
fn crate_ident(name: &str) -> String {
    name.replace('-', "_")
}

/// Quote a string with double quotes, escaping `"` and `\`. Cargo.toml
/// values are basic strings; we avoid TOML literal strings to keep
/// special characters working under all inputs.
fn quote_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
