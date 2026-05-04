//! Phase 8b: deterministic Cargo.toml emission from spec metadata.
//!
//! Reads `spec.toml` `[project]` + `architecture.toml` (`[platform]`,
//! `[runtime]`, `[logger]`, `[[external_deps]]`, `[msrv]`,
//! `[[features]]`) + module-level `external_deps[]`, and emits a
//! Cargo.toml whose content is a deterministic projection of the
//! spec.
//!
//! Phase 6.7 commitment: every Cargo dep is in the spec; Cargo.toml
//! is OUTPUT. This module is the OUTPUT half.
//!
//! Per D29 the analyzer doesn't generate Rust source code — but
//! Cargo.toml is configuration, not source. It's the same shape as
//! `extract` (deterministic projection in analyzer; Claude not
//! involved). The skill's `spec-gen` flow calls this subcommand to
//! emit Cargo.toml; Claude emits the .rs source files.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::layout::LayoutContext;

/// One Cargo.toml `[dependencies]` entry, normalized.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct CrateRef {
    #[serde(rename = "crate")]
    crate_name: String,
    version: String,
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
#[derive(Debug, Clone)]
pub struct RenderedCargoToml {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Render Cargo.toml for the project at `layout`. Dispatches across
/// four cases:
///
/// * **Workspace root** (`[workspace]` table set in spec.toml) →
///   [`render_workspace_root`]: emits `[workspace]` + `[workspace.package]`
///   + `[workspace.dependencies]`.
/// * **Workspace member** (no `[workspace]` table, but an ancestor
///   spec.toml carries one whose `members[].path` references this
///   directory) → renders the per-profile shape with workspace-aware
///   field inheritance: package fields owned by `[workspace_package]`
///   become `<field>.workspace = true`; deps with
///   `source = "workspace"` render as `{ workspace = true, ... }`.
/// * **Standalone library-api / firmware-layered project** → existing
///   render path, unchanged.
/// * **Anything else** → error.
pub fn render(layout: &LayoutContext) -> Result<RenderedCargoToml> {
    if layout.spec.workspace.is_some() {
        return render_workspace_root(layout);
    }
    let workspace_ctx = WorkspaceContext::find_for(layout)?;
    let profile = layout.spec.project.profile.as_str();
    match profile {
        "firmware-layered" => render_firmware(layout, workspace_ctx.as_ref()),
        "library-api" => render_library(layout, workspace_ctx.as_ref()),
        "audio-plugin" => render_audio_plugin(layout, workspace_ctx.as_ref()),
        "rust-workspace" => Err(anyhow!(
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

    let mut text = String::new();
    write_package_section(&mut text, project, /* rust_version */ None, workspace);
    text.push('\n');

    // [[bin]] for firmware-layered (binary projects).
    text.push_str("[[bin]]\n");
    text.push_str(&format!("name = {}\n", quote_str(&project.name)));
    text.push_str("path = \"src/main.rs\"\n");
    text.push('\n');

    write_dependencies_section(&mut text, &deps);

    Ok(RenderedCargoToml { text, warnings })
}

// ---------------------------------------------------------------------------
// library-api
// ---------------------------------------------------------------------------

fn render_library(
    layout: &LayoutContext,
    workspace: Option<&WorkspaceContext>,
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

    // Library architecture currently has no [[external_deps]] (per
    // Phase 6.7 plan: library-api's typed picks live at module
    // level), but accept it if present for future flexibility.
    for c in extract_crate_ref_array(arch.get("external_deps"))? {
        merge_dep(&mut deps, c, &mut warnings);
    }
    for c in collect_module_external_deps(layout)? {
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

    Ok(RenderedCargoToml { text, warnings })
}

// ---------------------------------------------------------------------------
// audio-plugin
// ---------------------------------------------------------------------------

/// Render Cargo.toml for an `audio-plugin` profile project.
///
/// Per Phase 15-A spike (D45): the plugin crate is always a `cdylib`
/// (the C-ABI surface VST3 and CLAP both require). When
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
) -> Result<RenderedCargoToml> {
    let project = &layout.spec.project;
    let arch = read_architecture(&layout.architecture_toml_path)?;

    let parsed_features = parse_library_features(arch.get("features"))?;

    // `[plugin].formats` drives the bin emission decision.
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
    let emit_standalone_bin = formats.iter().any(|f| f == "standalone");

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

    let mut text = String::new();
    write_package_section(&mut text, project, /* rust_version */ None, workspace);
    text.push('\n');

    // [lib] always cdylib for audio-plugin (D45). Plugin shells live in
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

    Ok(RenderedCargoToml { text, warnings })
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

    Ok(RenderedCargoToml { text, warnings })
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
            warnings.push(format!(
                "crate `{}` declared with conflicting versions ({} vs {}); using {}",
                c.crate_name, existing.version, c.version, existing.version,
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
    // today (could become a per-crate override field in a future phase).
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
    text.push_str("[dependencies]\n");
    for (name, c) in deps {
        text.push_str(&format!("{} = {}\n", name, render_dep_value(c)));
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
    if !has_extras {
        return quote_str(&c.version);
    }
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!("version = {}", quote_str(&c.version)));
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
