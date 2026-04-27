//! Validate-pipeline orchestrator (Phase 2 of the blocks + bridges + profiles arc).
//!
//! Layered:
//!   1. [`validate_layout`] — discovers `spec.toml`, builds a
//!      [`LayoutContext`], enumerates the `specs/specdev/` tree, runs the
//!      aggregator `$ref` resolver and reduction loader, and parses
//!      `domain.toml` for graph-check entries.
//!   2. [`validate_modules`] — compiles the aggregator schema, resolves the
//!      effective open-vocabulary kind set from the project's reductions,
//!      validates every module TOML under `specs/modules/` against the
//!      compiled aggregated schema, and resolves the kind references in
//!      every `domain.toml` graph-check entry against the effective set.

use std::path::Path;

use anyhow::Result;
use tracing::{event, instrument, Level};

use crate::aggregator;
use crate::domain::{DomainConfig, GraphCheck, Severity};
use crate::kinds;
use crate::layout::{LayoutContext, LayoutKind};
use crate::reductions;
use crate::report::{Finding, Report};
use crate::resolved::EffectiveKindSet;

/// Legacy compatibility shim — old callers expected an `Inputs` struct. The
/// new shape is a [`LayoutContext`]; this alias keeps the public API surface
/// recognizable while we land the rewrite.
pub type Inputs = LayoutContext;

/// Library-friendly entry point. Discover `spec.toml` from `start` (or from
/// `explicit_spec_path` if set) and run validation end-to-end.
pub fn run_project(start: &Path, explicit_spec_path: Option<&Path>) -> Result<Report> {
    let layout = LayoutContext::discover(start, explicit_spec_path)?;
    run(&layout)
}

#[instrument(level = "info", skip(layout), fields(project = %layout.project_root.display()))]
pub fn run(layout: &LayoutContext) -> Result<Report> {
    let mut report = Report::default();
    validate_layout(layout, &mut report)?;
    validate_modules(layout, &mut report)?;
    Ok(report)
}

/// Phase 1 layout pass: enumerate, aggregator, reductions, domain.toml.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_layout(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    let inv = layout.enumerate();
    for entry in &inv.directories {
        if entry.exists {
            report.push(Finding::new(
                "LAYOUT-002",
                Severity::Info,
                format!("present: {}", entry.label),
            ));
        } else {
            report.push(Finding::new(
                "LAYOUT-003",
                Severity::Warning,
                format!(
                    "missing: {} (expected at {})",
                    entry.label,
                    entry.path.display()
                ),
            ));
        }
    }
    for entry in &inv.files {
        if entry.exists {
            report.push(Finding::new(
                "LAYOUT-002",
                Severity::Info,
                format!("present: {}", entry.label),
            ));
        } else {
            // Skip the missing-warning for the codegen <lang> directory: it
            // is profile-driven and the project may not have authored
            // codegen yet.
            if entry.kind == LayoutKind::Directory
                && entry.label.starts_with("specs/specdev/codegen/")
            {
                continue;
            }
            report.push(Finding::new(
                "LAYOUT-003",
                Severity::Warning,
                format!(
                    "missing: {} (expected at {})",
                    entry.label,
                    entry.path.display()
                ),
            ));
        }
    }
    if layout.codegen_dir.is_dir() {
        report.push(Finding::new(
            "LAYOUT-004",
            Severity::Info,
            format!(
                "codegen directory present for language `{}` at {}",
                layout.spec.project.language,
                layout.codegen_dir.display()
            ),
        ));
    } else {
        report.push(Finding::new(
            "LAYOUT-004",
            Severity::Warning,
            format!(
                "codegen directory absent for language `{}` (expected at {})",
                layout.spec.project.language,
                layout.codegen_dir.display()
            ),
        ));
    }

    // Aggregator $ref resolver.
    let _agg = aggregator::resolve(layout, report)?;

    // Reduction loader.
    let inv = reductions::load_all(layout)?;
    if let Some(p) = &inv.project {
        report.push(Finding::new(
            "REDUCTION-001",
            Severity::Info,
            format!(
                "loaded project reduction from {} ({} kind declaration(s); kind resolution deferred to Phase 2)",
                p.source_path.display(),
                p.kind_declarations.len()
            ),
        ));
    }
    for r in inv
        .blocks
        .iter()
        .chain(inv.bridges_canonical.iter())
        .chain(inv.bridges_local.iter())
    {
        report.push(Finding::new(
            "REDUCTION-001",
            Severity::Info,
            format!(
                "loaded {} reduction from {} ({} kind declaration(s); kind resolution deferred to Phase 2)",
                r.namespace.label(),
                r.source_path.display(),
                r.kind_declarations.len()
            ),
        ));
    }

    // domain.toml: parse, gather graph-check entries, do not validate kinds.
    if layout.domain_toml_path.is_file() {
        match DomainConfig::load(&layout.domain_toml_path) {
            Ok(dom) => {
                let count = dom.module.graph_checks.len() + dom.project.graph_checks.len();
                report.push(Finding::new(
                    "DOMAIN-001",
                    Severity::Info,
                    format!(
                        "loaded domain.toml ({} graph-check entries; kind validation deferred to Phase 2)",
                        count
                    ),
                ));
            }
            Err(e) => {
                report.push(
                    Finding::new(
                        "DOMAIN-002",
                        Severity::Error,
                        format!("failed to parse domain.toml: {e:#}"),
                    )
                    .with_file(layout.domain_toml_path.display().to_string()),
                );
            }
        }
    }

    Ok(())
}

/// Phase 2 module pass.
///
/// 1. Build the [`EffectiveKindSet`] via [`reductions::resolve`] and merge
///    its findings into `report` (METASCHEMA-001/002/003/004/005).
/// 2. Compile the aggregator schema via [`aggregator::compile`]; emit
///    AGGREGATOR-005 if compilation fails. Cycles surface as
///    AGGREGATOR-003 errors during the resolve step.
/// 3. Validate every module TOML under `specs/modules/` against the
///    compiled aggregated schema; emit `SCHEMA` findings.
/// 4. Resolve `domain.toml` graph-check kind references against the
///    [`EffectiveKindSet`]; emit DOMAIN-003 for unresolved references.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_modules(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    let inv = layout.enumerate();

    // Step 1: build effective kind set.
    let raw_inv = reductions::load_all(layout)?;
    let (kindset, kindset_findings) = reductions::resolve(&raw_inv);
    report.extend(kindset_findings);

    // Step 2: compile aggregator schema.
    let validator = aggregator::compile(layout, report)?;

    // Step 3: validate each module TOML.
    for module_path in &inv.modules {
        event!(
            Level::DEBUG,
            module = %module_path.display(),
            "validating_module"
        );
        let text = match std::fs::read_to_string(module_path) {
            Ok(t) => t,
            Err(e) => {
                report.push(
                    Finding::new(
                        "LAYOUT-005",
                        Severity::Error,
                        format!("failed to read module {}: {e}", module_path.display()),
                    )
                    .with_file(module_path.display().to_string()),
                );
                continue;
            }
        };
        let parsed: toml::Value = match toml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                report.push(
                    Finding::new(
                        "LAYOUT-005",
                        Severity::Error,
                        format!("module {} TOML parse error: {e}", module_path.display()),
                    )
                    .with_file(module_path.display().to_string()),
                );
                continue;
            }
        };
        let json = match serde_json::to_value(&parsed) {
            Ok(v) => v,
            Err(e) => {
                report.push(
                    Finding::new(
                        "LAYOUT-005",
                        Severity::Error,
                        format!("module {} TOML→JSON failed: {e}", module_path.display()),
                    )
                    .with_file(module_path.display().to_string()),
                );
                continue;
            }
        };
        if let Some(v) = &validator {
            for err in v.iter_errors(&json) {
                report.push(
                    Finding::new("SCHEMA", Severity::Error, format!("{err}"))
                        .with_file(module_path.display().to_string())
                        .with_pointer(err.instance_path().to_string()),
                );
            }
        } else {
            report.push(Finding::new(
                "LAYOUT-005",
                Severity::Info,
                format!(
                    "module {} (no aggregator schema available — module-level structural \
                     validation skipped)",
                    module_path.display()
                ),
            ));
        }
    }

    // Step 4: domain.toml graph-check resolution.
    if layout.domain_toml_path.is_file() {
        match DomainConfig::load(&layout.domain_toml_path) {
            Ok(dom) => {
                resolve_graph_check_kinds(
                    &dom.module.graph_checks,
                    &kindset,
                    &layout.domain_toml_path,
                    "module",
                    report,
                );
                resolve_graph_check_kinds(
                    &dom.project.graph_checks,
                    &kindset,
                    &layout.domain_toml_path,
                    "project",
                    report,
                );
            }
            Err(_) => {
                // The DOMAIN-002 parse error is already emitted by
                // validate_layout; don't double-report.
            }
        }
    }

    Ok(())
}

/// Resolve every kind-string field of every graph-check entry against
/// the [`EffectiveKindSet`]. The check's primitive `kind` field is *not*
/// a kind reference (it names the check primitive: `acyclic`,
/// `reachability`, etc.); the kind references live in the selector
/// fields. Phase 2's load-time resolution takes whichever fields hold
/// kind paths, parses them, and verifies they resolve in the effective
/// set. Unresolved → `DOMAIN-003`.
///
/// Selector fields treated as kind references (when non-empty):
/// `scope`, `field`, `source_field`, `target_field`, `nodes_field`,
/// `edges_from`, `edges_to`. The selectors as written in `domain.toml`
/// historically named entity/relation kinds; in the open-vocabulary
/// regime they may be qualified paths. Unrecognized fields are ignored
/// (forward-compatible with new graph-check primitives).
fn resolve_graph_check_kinds(
    checks: &[GraphCheck],
    kindset: &EffectiveKindSet,
    domain_toml_path: &Path,
    section: &str,
    report: &mut Report,
) {
    for (idx, check) in checks.iter().enumerate() {
        let candidates: [&Option<String>; 7] = [
            &check.scope,
            &check.field,
            &check.source_field,
            &check.target_field,
            &check.nodes_field,
            &check.edges_from,
            &check.edges_to,
        ];
        for candidate in candidates.iter().copied().flatten() {
            // A field selector like `[*].kind` or `name` is not a kind
            // reference; only strings that parse as a kind path are
            // resolved. Skip the rest silently.
            let Ok(parsed) = kinds::parse(candidate) else {
                continue;
            };
            let key = parsed.to_string();
            let found_entity =
                kindset.entity(&key).is_some() || kindset.canonical_roots.contains(&key);
            let found_relation =
                kindset.relation(&key).is_some() || kindset.canonical_relations.contains(&key);
            if !(found_entity || found_relation) {
                report.push(
                    Finding::new(
                        "DOMAIN-003",
                        Severity::Error,
                        format!(
                            "domain.toml [{section}] graph_check #{idx} (rule `{}`): \
                             kind reference `{candidate}` is not in the effective kind set",
                            check.rule.as_deref().unwrap_or("?"),
                        ),
                    )
                    .with_file(domain_toml_path.display().to_string()),
                );
            }
        }
    }
}
