//! Validate-pipeline orchestrator (Phase 1 of the blocks + bridges + profiles arc).
//!
//! Layered:
//!   1. [`validate_layout`] — runs unconditionally. Discovers `spec.toml`,
//!      builds a [`LayoutContext`], enumerates the new `specs/specdev/`
//!      tree, runs the aggregator `$ref` resolver, the reduction loader,
//!      and parses `domain.toml` for graph-check entries (without resolving
//!      kinds — that's Phase 2).
//!   2. [`validate_modules`] — Phase 1 stub. Enumerates module TOMLs under
//!      `specs/modules/` and emits one `LAYOUT-005 deferred to Phase 2`
//!      info per discovery. Phase 2 replaces this with the real path-
//!      qualified-kind structural validation.

use std::path::Path;

use anyhow::Result;
use tracing::{event, instrument, Level};

use crate::aggregator;
use crate::domain::{DomainConfig, Severity};
use crate::layout::{LayoutContext, LayoutKind};
use crate::reductions;
use crate::report::{Finding, Report};

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

/// Phase 1 module pass: enumerate only. Structural validation lands in Phase 2.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_modules(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    let inv = layout.enumerate();
    for module_path in &inv.modules {
        event!(
            Level::DEBUG,
            module = %module_path.display(),
            "module_enumerated_phase_1_stub"
        );
        report.push(Finding::new(
            "LAYOUT-005",
            Severity::Info,
            format!(
                "module {} (structural validation deferred to Phase 2)",
                module_path.display()
            ),
        ));
    }
    Ok(())
}
