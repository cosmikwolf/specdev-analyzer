//! Validate-pipeline orchestrator.
//!
//! Ties the pieces together: load spec.toml + domain.toml, parse every
//! TOML spec file into JSON, run structural validation via the schema
//! registry, compose the project view, then run graph-check primitives.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::config::{self, SpecConfig};
use crate::domain::{DomainConfig, GraphCheck};
use crate::graph;
use crate::report::{Finding, Report};
use crate::schema::{self, SchemaRegistry};

/// Everything the caller needs to hand over to validate a project.
pub struct Inputs {
    pub spec_file: PathBuf,
    pub project_root: PathBuf,
    pub domain_file: PathBuf,
    pub schema_dir: PathBuf,
}

pub fn resolve_inputs(start: &Path, domain_override: Option<&Path>) -> Result<Inputs> {
    let spec_file = config::find_spec_toml(start).with_context(|| {
        format!(
            "spec.toml not found searching up from `{}` — run /spec-init or cd into a project directory",
            start.display()
        )
    })?;
    let project_root = spec_file
        .parent()
        .context("spec.toml has no parent directory (internal error)")?
        .to_path_buf();

    let domain_file = match domain_override {
        Some(p) => p.to_path_buf(),
        None => default_domain_file(&project_root).with_context(|| {
            let domain = SpecConfig::load(&spec_file)
                .map(|c| c.project.domain)
                .unwrap_or_else(|_| "<unknown>".into());
            format!(
                "domain config not found for domain `{domain}`; pass --domain <path> or create specdev/domains/{domain}/domain.toml"
            )
        })?,
    };

    let cfg = SpecConfig::load(&spec_file)?;
    let dom = DomainConfig::load(&domain_file)?;
    // Ensure the domain file's declared name matches spec.toml's domain field.
    anyhow::ensure!(
        dom.domain.name == cfg.project.domain,
        "domain mismatch: spec.toml declares `{}` but {} is `{}`",
        cfg.project.domain,
        domain_file.display(),
        dom.domain.name
    );

    let schema_dir = schema::schemas_dir(&domain_file, &dom.schemas).with_context(|| {
        format!(
            "could not resolve schema directory from `{}` — `[schemas]` table must contain at least one relative path",
            domain_file.display()
        )
    })?;

    Ok(Inputs {
        spec_file,
        project_root,
        domain_file,
        schema_dir,
    })
}

/// Conventional lookup for the plugin domain config when the caller didn't
/// pass --domain. Walks up looking for a sibling `specdev/domains/<d>/domain.toml`.
fn default_domain_file(project_root: &Path) -> Option<PathBuf> {
    let cfg = SpecConfig::load(&project_root.join("spec.toml")).ok()?;
    let domain = &cfg.project.domain;
    let mut dir = project_root.to_path_buf();
    for _ in 0..10 {
        let candidate = dir
            .join("specdev")
            .join("domains")
            .join(domain)
            .join("domain.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        // also check same-parent layout: ../specdev/domains/...
        if let Some(parent) = dir.parent() {
            let side = parent
                .join("specdev")
                .join("domains")
                .join(domain)
                .join("domain.toml");
            if side.is_file() {
                return Some(side);
            }
            dir = parent.to_path_buf();
        } else {
            return None;
        }
    }
    None
}

pub fn run(inputs: &Inputs) -> Result<Report> {
    let cfg = SpecConfig::load(&inputs.spec_file)?;
    let dom = DomainConfig::load(&inputs.domain_file)?;
    let registry = SchemaRegistry::load_dir(&inputs.schema_dir)?;
    let mut report = Report::default();

    let specs_dir = inputs.project_root.join("specs");

    // ---- Structural validation + composition ----
    let mut modules: Vec<Value> = Vec::new();
    let modules_dir = specs_dir.join("modules");
    if modules_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&modules_dir)?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let value = load_toml_as_json(&path)?;
            let rel = relpath(&path, &inputs.project_root);
            report.extend(registry.validate("module", &value, &rel)?);
            modules.push(value);
        }
    }

    let mut requirements: Vec<Value> = Vec::new();
    let req_dir = specs_dir.join("requirements");
    if req_dir.is_dir() {
        for entry in std::fs::read_dir(&req_dir)?.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let value = load_toml_as_json(&path)?;
            let rel = relpath(&path, &inputs.project_root);
            report.extend(registry.validate("requirements", &value, &rel)?);
            if let Some(arr) = value.get("requirement").and_then(Value::as_array) {
                requirements.extend(arr.iter().cloned());
            }
        }
    }

    let mut architecture = json!({});
    let arch_file = specs_dir.join("architecture.toml");
    if arch_file.is_file() {
        let value = load_toml_as_json(&arch_file)?;
        let rel = relpath(&arch_file, &inputs.project_root);
        report.extend(registry.validate("architecture", &value, &rel)?);
        architecture = value;
    }

    // ---- Required metadata (META010) ----
    let meta_sev = dom.severity_of("META010");
    for m in &modules {
        let name = m.get("name").and_then(Value::as_str).unwrap_or("?");
        for cat in &dom.required_metadata.module {
            let present = m.get("metadata").and_then(|v| v.get(cat)).is_some();
            if !present {
                report.push(
                    Finding::new(
                        "META010",
                        meta_sev,
                        format!("module `{name}` missing required metadata category `{cat}`"),
                    )
                    .with_pointer(format!("/metadata/{cat}")),
                );
            }
        }
    }

    // ---- Module-level graph checks ----
    for (i, m) in modules.iter().enumerate() {
        let file = format!("module[{i}]");
        let sub = run_checks(m, &dom.module.graph_checks, &dom, &file);
        report.extend(sub);
    }

    // ---- Project composition ----
    let project = compose_project(&modules, &architecture, &requirements);

    // ---- Project-level graph checks ----
    let sub = run_checks(&project, &dom.project.graph_checks, &dom, "project");
    report.extend(sub);

    let _ = cfg; // spec config reserved for later phases (schema major etc.)
    Ok(report)
}

fn run_checks(root: &Value, checks: &[GraphCheck], dom: &DomainConfig, file: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for c in checks {
        let rule = c.rule.clone().unwrap_or_default();
        let sev = if rule.is_empty() {
            crate::domain::Severity::Error
        } else {
            dom.severity_of(&rule)
        };
        let scope = c.scope.as_deref().unwrap_or("module");
        let per = matches!(scope, "per_state_machine");

        let findings = match c.kind.as_str() {
            "unique" => {
                let Some(field) = &c.field else { continue };
                graph::unique(root, field, &rule, sev)
            }
            "reference_exists" => {
                let (Some(src), Some(tgt)) = (&c.source_field, &c.target_field) else {
                    continue;
                };
                if per {
                    graph::reference_exists_per(root, src, tgt, 0, &rule, sev)
                } else {
                    graph::reference_exists(root, src, tgt, &rule, sev)
                }
            }
            "reachability" => {
                let (Some(nodes), Some(ef), Some(et), Some(entry)) =
                    (&c.nodes_field, &c.edges_from, &c.edges_to, &c.entry_field)
                else {
                    continue;
                };
                if per {
                    graph::reachability_per(root, nodes, ef, et, entry, 0, &rule, sev)
                } else {
                    graph::reachability(root, nodes, ef, et, entry, &rule, sev)
                }
            }
            "acyclic" => {
                let (Some(ef), Some(et)) = (&c.edges_from, &c.edges_to) else {
                    continue;
                };
                graph::acyclic(root, ef, et, &rule, sev)
            }
            "layer_dependency" => {
                let (Some(ml), Some(md)) = (&c.module_layer_field, &c.module_deps_field) else {
                    continue;
                };
                graph::layer_dependency(root, ml, md, &dom.layers.allowed_deps, &rule, sev)
            }
            other => {
                out.push(Finding::new(
                    "INTERNAL",
                    crate::domain::Severity::Warning,
                    format!("unknown graph_check kind `{other}` in domain config"),
                ));
                continue;
            }
        };
        for mut f in findings {
            if f.file.is_none() {
                f.file = Some(file.to_string());
            }
            out.push(f);
        }
    }
    out
}

/// Build the composite project value that project-level graph checks inspect.
/// Adds `qualified_name` to each port so PORT030 checks work against
/// `architecture.connections[].from`.
fn compose_project(modules: &[Value], architecture: &Value, requirements: &[Value]) -> Value {
    let mods: Vec<Value> = modules
        .iter()
        .map(|m| {
            let mut m = m.clone();
            let name = m
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(ports) = m.get_mut("ports").and_then(Value::as_array_mut) {
                for p in ports.iter_mut() {
                    let pname = p
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if let Some(obj) = p.as_object_mut() {
                        obj.insert(
                            "qualified_name".to_string(),
                            Value::String(format!("{name}.{pname}")),
                        );
                    }
                }
            }
            m
        })
        .collect();

    json!({
        "modules": mods,
        "architecture": architecture,
        "requirements": requirements,
    })
}

fn load_toml_as_json(path: &Path) -> Result<Value> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing TOML {}", path.display()))?;
    let value = serde_json::to_value(&raw)
        .with_context(|| format!("converting TOML to JSON for {}", path.display()))?;
    Ok(value)
}

fn relpath(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

// Expose a helper used by integration tests that need to validate a single
// free-standing module file without a full project.
pub fn validate_standalone_module(module_file: &Path, schema_dir: &Path) -> Result<Report> {
    let registry = SchemaRegistry::load_dir(schema_dir)?;
    let value = load_toml_as_json(module_file)?;
    let mut report = Report::default();
    let file = module_file.display().to_string();
    report.extend(registry.validate("module", &value, &file)?);
    Ok(report)
}
