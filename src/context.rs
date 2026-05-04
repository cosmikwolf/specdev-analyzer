//! Phase 11.5: focused dependency cone for one module.
//!
//! `specdev-analyzer context --module <Name>` emits the file set + sibling
//! modules a Claude-orchestrating skill needs to read in order to author,
//! edit, or reverse one module — replacing the "read every file under
//! `specs/modules/`" pattern in `/spec-design`, `/spec-edit`, and
//! `/spec-reverse`.
//!
//! Scope: read-only graph projection over the existing reduction. No new
//! schema. No new graph-check primitive. The cone is one hop deep — a
//! skill that needs transitive context calls `context` again on the
//! next-hop module.
//!
//! See `specdev-next/19-analyzer-context-subcommand.md` for the full
//! design rationale.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::layout::LayoutContext;
use crate::lineage::Lineage;
use crate::reduction::{self, Graph};

/// Top-level YAML report. Field order is the public surface — skill
/// consumers parse this shape.
#[derive(Debug, Clone, Serialize)]
pub struct ContextReport {
    pub module: String,
    pub profile: String,
    pub language: String,
    /// `"ok"` when the named module exists; `"module-not-found"` when it
    /// does not (the `/spec-design` cold-start path — schema + READMEs
    /// are still useful to the caller).
    pub status: String,
    pub files: ContextFiles,
    pub related_modules: RelatedModules,
    pub related_files: Vec<String>,
    pub excluded_module_count: usize,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profile_specific: BTreeMap<String, serde_yaml::Value>,
}

/// File paths the consumer should read. Project-relative for paths under
/// the project; `${CLAUDE_PLUGIN_ROOT}/...` for paths under the plugin.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ContextFiles {
    pub schema: Vec<String>,
    pub block_readmes: Vec<String>,
    pub bridge_readmes: Vec<String>,
    pub profile_readme: String,
    pub module_conventions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub architecture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_module: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RelatedModules {
    /// Modules M depends on (`depends_on` Module → Module, M is `from`).
    pub upstream: Vec<String>,
    /// Modules that depend on M (reverse `depends_on`, M is `to`).
    pub downstream: Vec<String>,
    /// Modules referenced via `architecture.toml [[connections]]` whose
    /// endpoint references a port owned by M (firmware-layered).
    pub port_endpoints: Vec<String>,
    /// Modules that share a non-Module entity with M via a bridge
    /// relation (e.g. `peripheral_owned_by` — both endpoints' owning
    /// modules land here).
    pub binding_endpoints: Vec<String>,
    /// Modules that declare a hardware peripheral or pin name that
    /// overlaps with one of M's (firmware-layered).
    pub hardware_owners: Vec<String>,
    /// Modules referenced by M's `[[items]]` `kind = "re_export"`
    /// `target` field (library-api).
    pub re_export_targets: Vec<String>,
}

/// Library-friendly entry point. Discovers layout, runs reduction,
/// builds the cone, returns the report. Wraps any error from the
/// underlying pipeline.
pub fn run_context(layout: &LayoutContext, module: &str) -> Result<ContextReport> {
    let (graph, _report) = reduction::run_project(&layout.project_root, None)
        .context("running reduction for context cone")?;
    build_report(layout, module, &graph)
}

/// Build the report from a layout + a pre-computed graph. Split out
/// for tests that want to construct a graph directly.
pub fn build_report(
    layout: &LayoutContext,
    module: &str,
    graph: &Graph,
) -> Result<ContextReport> {
    let profile = layout.spec.project.profile.clone();
    let language = layout.spec.project.language.clone();

    let files = build_files(layout, module)?;
    let target_exists = graph
        .entities
        .iter()
        .any(|e| e.kind == "Module" && e.name == module);

    if !target_exists {
        // /spec-design cold-start — module not yet authored. Return
        // schema + READMEs + a count of existing modules so the caller
        // knows the project's scale without forcing a full read.
        let total_modules = graph
            .entities
            .iter()
            .filter(|e| e.kind == "Module")
            .count();
        return Ok(ContextReport {
            module: module.to_string(),
            profile,
            language,
            status: "module-not-found".to_string(),
            files,
            related_modules: RelatedModules::default(),
            related_files: Vec::new(),
            excluded_module_count: total_modules,
            profile_specific: BTreeMap::new(),
        });
    }

    let related = compute_related(layout, module, graph)?;
    let related_files = related_files_for(&related, layout);

    let total_modules = graph
        .entities
        .iter()
        .filter(|e| e.kind == "Module")
        .count();
    let related_count = related.all_modules().len();
    // Subtract 1 for M itself; never go negative.
    let excluded = total_modules
        .saturating_sub(related_count)
        .saturating_sub(1);

    let profile_specific = compute_profile_specific(layout, module, graph);

    Ok(ContextReport {
        module: module.to_string(),
        profile,
        language,
        status: "ok".to_string(),
        files,
        related_modules: related,
        related_files,
        excluded_module_count: excluded,
        profile_specific,
    })
}

fn build_files(layout: &LayoutContext, module: &str) -> Result<ContextFiles> {
    let mut schema: Vec<String> = Vec::new();
    if layout.module_schema_path.is_file() {
        schema.push(rel(&layout.module_schema_path, &layout.project_root));
    }
    if layout.architecture_schema_path.is_file() {
        schema.push(rel(&layout.architecture_schema_path, &layout.project_root));
    }
    if layout.requirements_schema_path.is_file() {
        schema.push(rel(&layout.requirements_schema_path, &layout.project_root));
    }
    // Inlined block schemas, lex-sorted by basename (read_dir order is
    // platform-dependent; sort to make the YAML byte-stable).
    let inv = layout.enumerate();
    for p in &inv.inlined_block_schemas {
        schema.push(rel(p, &layout.project_root));
    }
    for p in &inv.inlined_bridge_schemas {
        schema.push(rel(p, &layout.project_root));
    }

    let lineage = Lineage::load(&layout.specdev_root).unwrap_or_default();

    let mut block_readmes: Vec<String> = lineage
        .blocks
        .keys()
        .map(|name| format!("${{CLAUDE_PLUGIN_ROOT}}/blocks/{name}/README.md"))
        .collect();
    block_readmes.sort();

    let mut bridge_readmes: Vec<String> = lineage
        .bridges
        .keys()
        .map(|name| format!("${{CLAUDE_PLUGIN_ROOT}}/bridges/{name}/README.md"))
        .collect();
    bridge_readmes.sort();

    let profile = &layout.spec.project.profile;
    let language = &layout.spec.project.language;
    let profile_readme = format!("${{CLAUDE_PLUGIN_ROOT}}/profiles/{profile}/README.md");
    let module_conventions = format!(
        "${{CLAUDE_PLUGIN_ROOT}}/profiles/{profile}/codegen/{language}/module.conventions.md"
    );

    let architecture = if layout.architecture_toml_path.is_file() {
        Some(rel(&layout.architecture_toml_path, &layout.project_root))
    } else {
        None
    };

    let target_module_path = layout.modules_dir.join(format!("{module}.toml"));
    let target_module = if target_module_path.is_file() {
        Some(rel(&target_module_path, &layout.project_root))
    } else {
        None
    };

    Ok(ContextFiles {
        schema,
        block_readmes,
        bridge_readmes,
        profile_readme,
        module_conventions,
        architecture,
        target_module,
    })
}

impl RelatedModules {
    /// Set of every sibling module name across all buckets — used for
    /// `related_files` rollup and `excluded_module_count`.
    fn all_modules(&self) -> BTreeSet<String> {
        let mut s: BTreeSet<String> = BTreeSet::new();
        for v in [
            &self.upstream,
            &self.downstream,
            &self.port_endpoints,
            &self.binding_endpoints,
            &self.hardware_owners,
            &self.re_export_targets,
        ] {
            for n in v {
                s.insert(n.clone());
            }
        }
        s
    }
}

fn related_files_for(related: &RelatedModules, layout: &LayoutContext) -> Vec<String> {
    let mut out: Vec<String> = related
        .all_modules()
        .into_iter()
        .map(|name| {
            let p = layout.modules_dir.join(format!("{name}.toml"));
            rel(&p, &layout.project_root)
        })
        .collect();
    out.sort();
    out
}

fn compute_related(
    layout: &LayoutContext,
    module: &str,
    graph: &Graph,
) -> Result<RelatedModules> {
    // entity_owner: (qualified_kind, name) → owning module name. Built
    // by walking entities and reading the file stem from provenance for
    // entries whose source is `specs/modules/<Name>.toml`.
    let mut entity_owner: BTreeMap<(String, String), String> = BTreeMap::new();
    for e in &graph.entities {
        if let Some(owner) = module_from_provenance(&e.provenance.source_file) {
            entity_owner.insert((e.kind.clone(), e.name.clone()), owner);
        }
    }

    // Set-builder buckets — BTreeSet for natural lex ordering.
    let mut upstream: BTreeSet<String> = BTreeSet::new();
    let mut downstream: BTreeSet<String> = BTreeSet::new();
    let mut port_endpoints: BTreeSet<String> = BTreeSet::new();
    let mut binding_endpoints: BTreeSet<String> = BTreeSet::new();

    for r in &graph.relations {
        let m_is_from_module = r.from_kind == "Module" && r.from == module;
        let m_is_to_module = r.to_kind == "Module" && r.to == module;

        if r.from_kind == "Module" && r.to_kind == "Module" {
            // depends_on (or any Module→Module relation, e.g. extends
            // depends_on). Direction matters for upstream/downstream.
            if m_is_from_module && r.to != module {
                upstream.insert(r.to.clone());
            }
            if m_is_to_module && r.from != module {
                downstream.insert(r.from.clone());
            }
            continue;
        }

        // Bridge / cross-block relation. Endpoints are non-Module
        // entities; trace each through entity_owner. If M owns one
        // endpoint, the other endpoint's owner (if a different module)
        // is a binding_endpoint.
        let from_owner = entity_owner.get(&(r.from_kind.clone(), r.from.clone()));
        let to_owner = entity_owner.get(&(r.to_kind.clone(), r.to.clone()));

        if let (Some(f), Some(t)) = (from_owner, to_owner) {
            if f == module && t != module {
                binding_endpoints.insert(t.clone());
            }
            if t == module && f != module {
                binding_endpoints.insert(f.clone());
            }
        }
    }

    // Architecture connections — firmware-layered emits Connection
    // entities whose `from` and `to` attrs are "Module.port" strings.
    // Walk Connection entities and pluck the other-module-name when M
    // is on one side.
    for e in &graph.entities {
        if e.kind != "Connection" {
            continue;
        }
        let from_str = e
            .attrs
            .get("from")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let to_str = e
            .attrs
            .get("to")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let from_mod = from_str.split('.').next().unwrap_or("").to_string();
        let to_mod = to_str.split('.').next().unwrap_or("").to_string();
        if from_mod == module && to_mod != module && !to_mod.is_empty() {
            port_endpoints.insert(to_mod);
        } else if to_mod == module && from_mod != module && !from_mod.is_empty() {
            port_endpoints.insert(from_mod);
        }
    }

    // Hardware overlap — peripherals/pins by *local* name (the suffix
    // after `<module>.`). The reduction qualifies entity names with
    // their owning module so e.g. `A.shared_uart` and `B.shared_uart`
    // are distinct entities; the overlap signal we want is "two
    // modules declare a peripheral with the same local name."
    // Firmware-layered-specific (block names `<*>.Peripheral` /
    // `<*>.Pin`).
    let mut hardware_owners: BTreeSet<String> = BTreeSet::new();
    let mut my_hardware_locals: BTreeSet<(String, String)> = BTreeSet::new();
    for e in &graph.entities {
        if !is_hardware_kind(&e.kind) {
            continue;
        }
        let Some(owner) = entity_owner.get(&(e.kind.clone(), e.name.clone())) else {
            continue;
        };
        if owner != module {
            continue;
        }
        if let Some(local) = local_part(&e.name) {
            my_hardware_locals.insert((e.kind.clone(), local.to_string()));
        }
    }
    if !my_hardware_locals.is_empty() {
        for e in &graph.entities {
            if !is_hardware_kind(&e.kind) {
                continue;
            }
            let Some(local) = local_part(&e.name) else {
                continue;
            };
            if !my_hardware_locals.contains(&(e.kind.clone(), local.to_string())) {
                continue;
            }
            let Some(owner) = entity_owner.get(&(e.kind.clone(), e.name.clone())) else {
                continue;
            };
            if owner != module {
                hardware_owners.insert(owner.clone());
            }
        }
    }

    // Re-export targets — library-api modules carry [[items]] entries
    // with `kind = "re_export"` whose `target` is `<sibling>::<Item>`.
    // Read M's TOML directly (items[] aren't in the reduction graph).
    let mut re_export_targets: BTreeSet<String> = BTreeSet::new();
    let module_path = layout.modules_dir.join(format!("{module}.toml"));
    if module_path.is_file() {
        let text = std::fs::read_to_string(&module_path)
            .with_context(|| format!("reading {}", module_path.display()))?;
        if let Ok(parsed) = toml::from_str::<toml::Value>(&text) {
            if let Some(items) = parsed.get("items").and_then(toml::Value::as_array) {
                for it in items {
                    let kind = it.get("kind").and_then(toml::Value::as_str).unwrap_or("");
                    if kind != "re_export" {
                        continue;
                    }
                    let target = it.get("target").and_then(toml::Value::as_str).unwrap_or("");
                    let sibling = target.split("::").next().unwrap_or("").to_string();
                    if !sibling.is_empty() && sibling != module {
                        re_export_targets.insert(sibling);
                    }
                }
            }
        }
    }

    Ok(RelatedModules {
        upstream: upstream.into_iter().collect(),
        downstream: downstream.into_iter().collect(),
        port_endpoints: port_endpoints.into_iter().collect(),
        binding_endpoints: binding_endpoints.into_iter().collect(),
        hardware_owners: hardware_owners.into_iter().collect(),
        re_export_targets: re_export_targets.into_iter().collect(),
    })
}

/// True for entity kinds that represent hardware resources we want to
/// flag overlap on. Conservative: matches `*.Peripheral` / `*.Pin`.
fn is_hardware_kind(kind: &str) -> bool {
    if let Some((_, suffix)) = kind.rsplit_once('.') {
        matches!(suffix, "Peripheral" | "Pin")
    } else {
        false
    }
}

/// Strip the `<owning-module>.` prefix from a qualified entity name
/// (the reduction emits `${parent.name}.${.name}` for block-contributed
/// entities). Returns `None` for unqualified names.
fn local_part(qualified_name: &str) -> Option<&str> {
    qualified_name.split_once('.').map(|(_, rest)| rest)
}

fn module_from_provenance(source_file: &str) -> Option<String> {
    // source_file is project-relative, forward-slashed. Module specs
    // live at `specs/modules/<Name>.toml`. Anything else (architecture,
    // requirements) is not a module.
    let normalized = source_file.replace('\\', "/");
    let prefix = "specs/modules/";
    let rest = normalized.strip_prefix(prefix)?;
    let stem = rest.strip_suffix(".toml")?;
    if stem.contains('/') {
        return None;
    }
    Some(stem.to_string())
}

fn compute_profile_specific(
    layout: &LayoutContext,
    module: &str,
    graph: &Graph,
) -> BTreeMap<String, serde_yaml::Value> {
    let mut out: BTreeMap<String, serde_yaml::Value> = BTreeMap::new();

    // Look up the Module entity for M and surface attrs that the
    // skills' impact-analysis steps consult.
    let ent = graph
        .entities
        .iter()
        .find(|e| e.kind == "Module" && e.name == module);
    let Some(ent) = ent else {
        return out;
    };

    match layout.spec.project.profile.as_str() {
        "firmware-layered" => {
            if let Some(layer) = ent.attrs.get("layer").and_then(|v| v.as_str()) {
                out.insert(
                    "layer".to_string(),
                    serde_yaml::Value::String(layer.to_string()),
                );
            }
            // Hardware resource list — peripheral + pin names owned by M.
            let mut peripherals: Vec<String> = Vec::new();
            let mut pins: Vec<String> = Vec::new();
            for e in &graph.entities {
                let Some(owner) = module_from_provenance(&e.provenance.source_file) else {
                    continue;
                };
                if owner != module {
                    continue;
                }
                let suffix = e.kind.rsplit_once('.').map(|(_, s)| s).unwrap_or("");
                match suffix {
                    "Peripheral" => peripherals.push(e.name.clone()),
                    "Pin" => pins.push(e.name.clone()),
                    _ => {}
                }
            }
            peripherals.sort();
            pins.sort();
            if !peripherals.is_empty() || !pins.is_empty() {
                let mut hw: BTreeMap<String, serde_yaml::Value> = BTreeMap::new();
                hw.insert(
                    "peripherals".to_string(),
                    serde_yaml::Value::Sequence(
                        peripherals
                            .into_iter()
                            .map(serde_yaml::Value::String)
                            .collect(),
                    ),
                );
                hw.insert(
                    "pins".to_string(),
                    serde_yaml::Value::Sequence(
                        pins.into_iter().map(serde_yaml::Value::String).collect(),
                    ),
                );
                let mut hw_yaml = serde_yaml::Mapping::new();
                for (k, v) in hw {
                    hw_yaml.insert(serde_yaml::Value::String(k), v);
                }
                out.insert(
                    "hardware_resources".to_string(),
                    serde_yaml::Value::Mapping(hw_yaml),
                );
            }
        }
        "library-api" => {
            if let Some(vis) = ent.attrs.get("visibility").and_then(|v| v.as_str()) {
                out.insert(
                    "visibility".to_string(),
                    serde_yaml::Value::String(vis.to_string()),
                );
            }
        }
        _ => {}
    }

    out
}

fn rel(full: &Path, root: &Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Convenience: serialize a [`ContextReport`] to deterministic YAML.
pub fn to_yaml(report: &ContextReport) -> Result<String> {
    serde_yaml::to_string(report).context("serializing context report to YAML")
}

/// Convenience: write the report to a file (or stdout if `path` is
/// None). Used by the CLI dispatch.
pub fn emit(report: &ContextReport, path: Option<&PathBuf>) -> Result<()> {
    let yaml = to_yaml(report)?;
    match path {
        Some(p) => {
            std::fs::write(p, &yaml)
                .with_context(|| format!("writing context to {}", p.display()))?;
            eprintln!("wrote context to {}", p.display());
        }
        None => print!("{yaml}"),
    }
    Ok(())
}
