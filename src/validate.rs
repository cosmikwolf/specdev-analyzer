//! Validate-pipeline orchestrator.
//!
//! Ties the pieces together: load spec.toml + domain.toml, parse every
//! TOML spec file into JSON, run structural validation via the schema
//! registry, compose the project view, then run graph-check primitives.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::config::{self, SpecConfig};
use crate::domain::{DomainConfig, GraphCheck};
use crate::graph;
use crate::packages;
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
    let mut module_packages: Vec<Option<String>> = Vec::new();
    let modules_dir = specs_dir.join("modules");
    let (module_files, depth_findings) =
        packages::collect_module_files(&modules_dir, &inputs.project_root);
    report.extend(depth_findings);

    // PKG040 / PKG041 placement findings.
    let pkg040_sev = dom.severity_of("PKG040");
    let pkg041_sev = dom.severity_of("PKG041");
    report.extend(packages::package_placement_findings(
        &module_files,
        &cfg.packages,
        &inputs.project_root,
        pkg040_sev,
        pkg041_sev,
    ));

    // PKG042 — declared source_path must exist.
    let pkg042_sev = dom.severity_of("PKG042");
    report.extend(packages::package_source_path_findings(
        &cfg.packages,
        &inputs.project_root,
        pkg042_sev,
    ));

    let declared: std::collections::HashSet<&str> =
        cfg.packages.iter().map(|p| p.name.as_str()).collect();
    for mf in &module_files {
        let value = load_toml_as_json(&mf.path)?;
        let rel = relpath(&mf.path, &inputs.project_root);
        report.extend(registry.validate("module", &value, &rel)?);
        // Only treat a subdirectory as a real package if it was declared;
        // undeclared subdirs (PKG040) degrade to unpackaged downstream.
        let pkg = match &mf.package {
            Some(p) if declared.contains(p.as_str()) => Some(p.clone()),
            _ => None,
        };
        module_packages.push(pkg);
        modules.push(value);
    }

    // Build name → package map for reference normalization.
    let mut pkg_by_name: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (m, pkg) in modules.iter().zip(module_packages.iter()) {
        if let Some(name) = m.get("name").and_then(Value::as_str) {
            pkg_by_name.insert(name.to_string(), pkg.clone());
        }
    }

    // Normalize `depends_on[]` references to fully-qualified form.
    let pkg043_sev = dom.severity_of("PKG043");
    let pkg044_sev = dom.severity_of("PKG044");
    report.extend(packages::normalize_depends_on(
        &mut modules,
        &cfg.packages,
        &pkg_by_name,
        pkg043_sev,
        pkg044_sev,
    ));

    let mut requirements: Vec<Value> = Vec::new();
    let mut sources: Vec<Value> = Vec::new();
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
            if let Some(arr) = value.get("source").and_then(Value::as_array) {
                sources.extend(arr.iter().cloned());
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
        if !cfg.packages.is_empty() {
            report.extend(packages::normalize_architecture(
                &mut architecture,
                &modules,
                &cfg.packages,
                &pkg_by_name,
                pkg043_sev,
                pkg044_sev,
            ));
        }
    }

    // ---- Domain-declared collections ----
    let mut extra_collections: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for coll in &dom.collections {
        let mut items: Vec<Value> = Vec::new();
        let dir = specs_dir.join(&coll.dir);
        if dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&dir)?.filter_map(Result::ok).collect();
            entries.sort_by_key(|e| e.path());
            for entry in entries {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                let value = load_toml_as_json(&path)?;
                let rel = relpath(&path, &inputs.project_root);
                report.extend(registry.validate(&coll.schema, &value, &rel)?);
                items.push(value);
            }
        }
        extra_collections.insert(coll.name.clone(), items);
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
    let project = compose_project(
        &modules,
        &module_packages,
        &architecture,
        &requirements,
        &sources,
        &extra_collections,
    );

    // ---- Project-level graph checks ----
    let sub = run_checks(&project, &dom.project.graph_checks, &dom, "project");
    report.extend(sub);

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
fn compose_project(
    modules: &[Value],
    module_packages: &[Option<String>],
    architecture: &Value,
    requirements: &[Value],
    sources: &[Value],
    extra: &BTreeMap<String, Vec<Value>>,
) -> Value {
    let mods: Vec<Value> = modules
        .iter()
        .zip(module_packages.iter())
        .map(|(m, pkg)| {
            let mut m = m.clone();
            let bare = m
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let qualified = packages::qualify(pkg.as_deref(), &bare);
            // Rewrite `name` in the composed view to the qualified form so
            // every `modules[].name` target matches qualified `depends_on[]`
            // and qualified architecture endpoints. The per-module extract
            // output still uses the bare `name`.
            if let Some(obj) = m.as_object_mut() {
                obj.insert("name".to_string(), Value::String(qualified.clone()));
            }
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
                            Value::String(format!("{qualified}.{pname}")),
                        );
                    }
                }
            }
            m
        })
        .collect();

    let mut out = json!({
        "modules": mods,
        "architecture": architecture,
        "requirements": requirements,
        "sources": sources,
    });
    if let Some(obj) = out.as_object_mut() {
        for (k, v) in extra {
            obj.insert(k.clone(), Value::Array(v.clone()));
        }
    }
    out
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
