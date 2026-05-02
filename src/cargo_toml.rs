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
struct CrateRef {
    #[serde(rename = "crate")]
    crate_name: String,
    version: String,
    #[serde(default)]
    source: Option<CrateSource>,
    #[serde(default)]
    features: Option<Vec<String>>,
    #[serde(default)]
    default_features: Option<bool>,
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

/// Render Cargo.toml for the project at `layout`, choosing the
/// firmware-layered or library-api shape based on `[project].profile`.
pub fn render(layout: &LayoutContext) -> Result<RenderedCargoToml> {
    let profile = layout.spec.project.profile.as_str();
    match profile {
        "firmware-layered" => render_firmware(layout),
        "library-api" => render_library(layout),
        other => Err(anyhow!(
            "cargo-toml emission for profile `{other}` is not implemented; \
             supported: firmware-layered, library-api",
        )),
    }
}

// ---------------------------------------------------------------------------
// firmware-layered
// ---------------------------------------------------------------------------

fn render_firmware(layout: &LayoutContext) -> Result<RenderedCargoToml> {
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
    write_package_section(&mut text, project, /* rust_version */ None);
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

fn render_library(layout: &LayoutContext) -> Result<RenderedCargoToml> {
    let project = &layout.spec.project;
    let arch = read_architecture(&layout.architecture_toml_path)?;

    let msrv = arch
        .get("msrv")
        .and_then(toml::Value::as_str)
        .map(String::from);

    let features = parse_library_features(arch.get("features"))?;

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
    write_package_section(&mut text, project, msrv.as_deref());
    text.push('\n');

    text.push_str("[lib]\n\n");

    if !features.is_empty() {
        text.push_str("[features]\n");
        for (name, deps) in &features {
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

fn parse_library_features(value: Option<&toml::Value>) -> Result<BTreeMap<String, Vec<String>>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let Some(arr) = value.and_then(toml::Value::as_array) else {
        return Ok(out);
    };
    for item in arr {
        let Some(t) = item.as_table() else { continue };
        let Some(name) = t.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let mut deps: Vec<String> = t
            .get("depends_on")
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        deps.sort();
        deps.dedup();
        out.insert(name.to_string(), deps);
    }
    Ok(out)
}

fn write_package_section(
    text: &mut String,
    project: &crate::config::ProjectSection,
    rust_version: Option<&str>,
) {
    text.push_str("[package]\n");
    text.push_str(&format!("name = {}\n", quote_str(&project.name)));
    let edition = project.edition.as_deref().unwrap_or("2021");
    text.push_str(&format!("edition = {}\n", quote_str(edition)));
    if let Some(rv) = rust_version {
        text.push_str(&format!("rust-version = {}\n", quote_str(rv)));
    }
    if let Some(license) = &project.license {
        text.push_str(&format!("license = {}\n", quote_str(license)));
    }
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
    if let Some(CrateSource::Tagged(CrateSourceTag::Workspace)) = c.source {
        return "{ workspace = true }".into();
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
        || c.default_features.is_some();
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
