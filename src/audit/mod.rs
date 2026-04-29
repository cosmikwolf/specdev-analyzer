//! Audit pipeline — compare extracted module specs against source code.

pub mod code_parser;
pub mod compare;
pub mod construct;
pub mod source_map;
mod util;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::SpecConfig;
use crate::lang::{self, LanguageAdapter};
use crate::layout::LayoutContext;

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),

    #[error("tree-sitter: {0}")]
    TreeSitter(String),

    #[error("i/o: {0}")]
    Io(#[from] std::io::Error),

    #[error("macro expansion failed: {0}")]
    ExpandFailed(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditReport {
    pub modules: Vec<ModuleAudit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleAudit {
    pub module_name: String,
    pub source_file: Option<PathBuf>,
    pub items: Vec<AuditItem>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum AuditItem {
    Match {
        kind: String,
        name: String,
    },
    Missing {
        kind: String,
        name: String,
        detail: String,
    },
    Mismatch {
        kind: String,
        name: String,
        spec_detail: String,
        code_detail: String,
    },
    Uncovered {
        kind: String,
        name: String,
        line: usize,
    },
}

pub struct AuditOptions<'a> {
    pub show_uncovered: bool,
    pub expand: bool,
    pub module_filter: Option<&'a str>,
    /// Optional spec-type → code-type translation.
    pub type_map: Option<&'a HashMap<String, String>>,
}

/// Audit a project: parse source for each module and compare against the
/// canonical extraction. Does not itself trigger validation/extraction — the
/// caller owns that pipeline.
pub fn run(
    project_root: &Path,
    spec_cfg: &SpecConfig,
    modules: &[crate::extract::ExtractedModule],
    options: AuditOptions<'_>,
) -> Result<AuditReport> {
    let language = &spec_cfg.project.language;
    let adapter = lang::adapter_for(language).with_context(|| {
        format!(
            "no language adapter for `{language}`; supported: rust, c, cpp. \
             Fix `[project].language` in spec.toml or add an adapter."
        )
    })?;

    let project_source_root = spec_cfg.paths.source.clone();
    // Phase 1: source-layout convention is "flat". Phase 2 may add layout
    // hints back via profile or spec.toml as profiles surface real needs.
    let layout = "flat".to_string();

    let mut out = Vec::new();
    for em in modules {
        let name = &em.name;
        let module = &em.value;
        if let Some(filter) = options.module_filter {
            if name != filter {
                continue;
            }
        }

        // Phase 1: per-package source_path is gone (multi-package machinery
        // is per-package independent under doc 15 §9.2). Use the
        // project-wide source root for every module.
        let source_root: &str = project_source_root.as_str();

        let path = source_map::resolve_source_path(
            module,
            adapter.as_ref(),
            project_root,
            source_root,
            &layout,
        );

        let items = match &path {
            Some(p) => {
                let source = read_source(p, adapter.as_ref(), options.expand)?;
                let constructs = code_parser::parse_source(&source, adapter.as_ref(), None)
                    .map_err(|e| anyhow::anyhow!("parsing `{}`: {e}", p.display()))?;
                compare::compare_module(
                    module,
                    &constructs,
                    options.show_uncovered,
                    options.type_map,
                )
            }
            None => vec![AuditItem::Missing {
                kind: "file".into(),
                name: name.clone(),
                detail: format!("no source file found for module `{name}`"),
            }],
        };

        out.push(ModuleAudit {
            module_name: name.clone(),
            source_file: path,
            items,
        });
    }

    Ok(AuditReport { modules: out })
}

fn read_source(path: &Path, adapter: &dyn LanguageAdapter, expand: bool) -> Result<String> {
    if expand {
        if let Some(text) = try_expand(path, adapter.name()) {
            return Ok(text);
        }
        eprintln!(
            "warning: macro expansion failed for {}; using raw source",
            path.display()
        );
    }
    std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))
}

fn try_expand(path: &Path, language: &str) -> Option<String> {
    match language {
        "c" | "cpp" => {
            let cc = if language == "cpp" { "c++" } else { "cc" };
            let out = std::process::Command::new(cc)
                .args(["-E", &path.to_string_lossy()])
                .output()
                .ok()?;
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).to_string())
            } else {
                None
            }
        }
        "rust" => {
            let out = std::process::Command::new("cargo")
                .args(["expand", "--lib"])
                .output()
                .ok()?;
            if out.status.success() {
                Some(String::from_utf8_lossy(&out.stdout).to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

pub fn format_text(report: &AuditReport) -> String {
    let mut out = String::new();
    for m in &report.modules {
        let file = m
            .source_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not found".into());
        out.push_str(&format!("{} ({}):\n", m.module_name, file));
        for item in &m.items {
            match item {
                AuditItem::Match { kind, name } => {
                    out.push_str(&format!("  ✓ {kind} {name}\n"));
                }
                AuditItem::Missing { kind, name, detail } => {
                    out.push_str(&format!("  + {kind} {name} — {detail}\n"));
                }
                AuditItem::Mismatch {
                    kind,
                    name,
                    spec_detail,
                    code_detail,
                } => {
                    out.push_str(&format!(
                        "  ~ {kind} {name} — spec: {spec_detail}, code: {code_detail}\n"
                    ));
                }
                AuditItem::Uncovered { kind, name, line } => {
                    out.push_str(&format!("  ? {kind} {name} (line {line})\n"));
                }
            }
        }
        out.push('\n');
    }
    out
}

pub struct AuditSummary {
    pub matches: usize,
    pub missing: usize,
    pub mismatches: usize,
    pub uncovered: usize,
}

impl AuditReport {
    pub fn summary(&self) -> AuditSummary {
        let mut s = AuditSummary {
            matches: 0,
            missing: 0,
            mismatches: 0,
            uncovered: 0,
        };
        for m in &self.modules {
            for i in &m.items {
                match i {
                    AuditItem::Match { .. } => s.matches += 1,
                    AuditItem::Missing { .. } => s.missing += 1,
                    AuditItem::Mismatch { .. } => s.mismatches += 1,
                    AuditItem::Uncovered { .. } => s.uncovered += 1,
                }
            }
        }
        s
    }
}

/// End-to-end convenience: validate → extract → audit.
pub fn run_project(layout: &LayoutContext, options: AuditOptions<'_>) -> Result<AuditReport> {
    let extraction = crate::extract::run(layout)?;
    run(
        &layout.project_root,
        &layout.spec,
        &extraction.modules,
        options,
    )
}
