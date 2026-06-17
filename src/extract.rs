//! Extract pipeline.
//!
//! Validates the project, runs the open-vocabulary
//! reduction executor against the resolved [`EffectiveKindSet`], and
//! produces canonical YAML for codegen consumption:
//!
//! * `<output>/modules/<Name>.yaml` — one per source module TOML.
//! * `<output>/architecture.yaml` — the full executor graph
//!   (`entities[]`, `relations[]`).
//! * `<output>/requirements.yaml` — graph filtered to entities whose
//!   `extends` chain reaches the canonical `statement` root.
//!
//! Determinism: serialization uses BTreeMap-backed structs so key order
//! is stable across runs. Modules are written in lex order.
//!
//! Validation gating: extract bails if the validation report contains
//! any errors. The caller is expected to surface the report.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tracing::{event, instrument, Level};

use crate::layout::LayoutContext;
use crate::reduction;

#[derive(Debug, Default)]
pub struct ExtractionResult {
    pub modules: Vec<ExtractedModule>,
    pub architecture: Value,
    pub requirements: Value,
    pub collections: BTreeMap<String, Vec<(String, Value)>>,
}

#[derive(Debug)]
pub struct ExtractedModule {
    pub name: String,
    pub package: Option<String>,
    pub value: Value,
}

/// Library-friendly entry point. Discovers `spec.toml`, runs validation
/// (gating extract on a clean report), executes the reduction, and
/// builds an [`ExtractionResult`].
pub fn run_project(start: &Path, explicit_spec_path: Option<&Path>) -> Result<ExtractionResult> {
    let layout = LayoutContext::discover(start, explicit_spec_path)?;
    run(&layout)
}

#[instrument(level = "info", skip(layout), fields(project = %layout.project_root.display()))]
pub fn run(layout: &LayoutContext) -> Result<ExtractionResult> {
    let report = crate::validate::run(layout)?;
    if report.has_errors() {
        return Err(anyhow!(
            "extract refuses to run: validation produced {} error(s). Run \
             `specdev-analyzer validate` to inspect.",
            report.counts().0
        ));
    }

    // Materialize the project graph. Re-runs the resolver/executor; the
    // validate pass already exercised them but didn't return the graph.
    let inv = crate::reductions::load_all(layout)?;
    let (kindset, _findings) = crate::reductions::resolve(&inv);
    let mut exec_report = crate::report::Report::default();
    let graph = reduction::execute_kindset(&kindset, layout, &mut exec_report)?;

    let architecture =
        serde_json::to_value(&graph).with_context(|| "serializing executor graph to JSON")?;

    // Requirements view: keep only entities whose extends chain reaches
    // `statement`; relations are passed through unchanged so callers can
    // see satisfies edges between modules and requirements.
    let mut requirements = architecture.clone();
    if let Some(entities) = requirements
        .get_mut("entities")
        .and_then(|v| v.as_array_mut())
    {
        entities.retain(|e| {
            e.get("extends")
                .and_then(|v| v.as_str())
                .is_some_and(|s| s == "statement")
        });
    }

    // Modules: walk specs/modules/*.toml, parse each.
    let modules = collect_modules(layout)?;

    Ok(ExtractionResult {
        modules,
        architecture,
        requirements,
        collections: BTreeMap::new(),
    })
}

/// Write the extraction result as canonical YAML under `output_dir`.
/// Returns the list of files produced for caller logging.
pub fn write(result: &ExtractionResult, output_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("creating {}", output_dir.display()))?;

    let modules_dir = output_dir.join("modules");
    std::fs::create_dir_all(&modules_dir)
        .with_context(|| format!("creating {}", modules_dir.display()))?;
    for em in &result.modules {
        let path = modules_dir.join(format!("{}.yaml", em.name));
        let yaml = serde_yaml::to_string(&em.value)
            .with_context(|| format!("serializing module {}", em.name))?;
        std::fs::write(&path, yaml).with_context(|| format!("writing {}", path.display()))?;
        event!(Level::DEBUG, module = %em.name, path = %path.display(), "module_written");
        written.push(path);
    }

    let arch_path = output_dir.join("architecture.yaml");
    let arch_yaml = serde_yaml::to_string(&result.architecture)
        .with_context(|| "serializing architecture.yaml")?;
    std::fs::write(&arch_path, arch_yaml)
        .with_context(|| format!("writing {}", arch_path.display()))?;
    written.push(arch_path);

    let req_path = output_dir.join("requirements.yaml");
    let req_yaml = serde_yaml::to_string(&result.requirements)
        .with_context(|| "serializing requirements.yaml")?;
    std::fs::write(&req_path, req_yaml)
        .with_context(|| format!("writing {}", req_path.display()))?;
    written.push(req_path);

    Ok(written)
}

fn collect_modules(layout: &LayoutContext) -> Result<Vec<ExtractedModule>> {
    let inv = layout.enumerate();
    let mut out = Vec::new();
    for path in &inv.modules {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading module {}", path.display()))?;
        let toml_value: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing module {}", path.display()))?;
        let json: Value = serde_json::to_value(&toml_value)
            .with_context(|| format!("converting module {} to JSON", path.display()))?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.push(ExtractedModule {
            name,
            package: None,
            value: json,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
