//! Coverage report — requirements → modules.
//!
//! Walks `specs/requirements.toml` for every requirement id and
//! `specs/modules/*.toml` for every module's `satisfies` (module-level)
//! and items[]'s `satisfies` (item-level). Emits a structured matrix
//! mapping each requirement id to the list of modules that satisfy it.
//! Requirements with no satisfiers appear in a separate `orphans` list.
//!
//! Output formats: YAML (machine-readable, default; the `report coverage`
//! subcommand's primary consumer is CI / downstream skill tooling) and
//! Markdown (human-readable opt-in via `--format markdown`).
//!
//! Replaces the coverage-matrix half of the retired `skeleton.rs`
//! `render_handoff_md` function. The remaining HANDOFF.md sections
//! (skeleton inventory, plugin gaps, suggested next steps) retired
//! alongside the skeleton feature.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::entries::{EntryKind, EntryStore};
use crate::items::sync::parse_spec_items;
use crate::layout::LayoutContext;

#[derive(Debug, Clone, Serialize)]
pub struct CoverageReport {
    /// Requirement id → satisfying module names, in declaration order
    /// of the source `[[requirement]]` entries. Modules within a row
    /// are sorted lex.
    pub matrix: Vec<CoverageRow>,
    /// Requirement ids with no satisfying module.
    pub orphans: Vec<String>,
    /// Retired requirement ids: never in the matrix or `orphans`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retired: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CoverageRow {
    pub requirement: String,
    pub satisfied_by: Vec<String>,
}

#[derive(Debug)]
pub enum CoverageFormat {
    Yaml,
    Markdown,
}

impl CoverageFormat {
    pub fn parse(s: &str) -> Result<Self> {
        match s {
            "yaml" => Ok(CoverageFormat::Yaml),
            "markdown" | "md" => Ok(CoverageFormat::Markdown),
            other => Err(anyhow!(
                "[COVERAGE-001] unknown --format `{other}`; valid values: `yaml`, `markdown`"
            )),
        }
    }
}

/// Build the coverage report from a discovered project layout.
///
/// Returns an empty report (empty matrix, empty orphans) when the
/// project has no `requirements.toml` — this is expected for projects
/// that haven't authored requirements yet, not an error.
pub fn build(layout: &LayoutContext) -> Result<CoverageReport> {
    let requirements = load_requirement_ids(layout)?;
    let modules = load_modules(layout)?;

    let mut matrix: Vec<CoverageRow> = Vec::with_capacity(requirements.len());
    let mut orphans: Vec<String> = Vec::new();

    for req in &requirements {
        let mut satisfiers: Vec<String> = Vec::new();
        for m in &modules {
            let module_satisfies = m.satisfies.iter().any(|s| s == req);
            let item_satisfies = m.item_satisfies.iter().any(|s| s == req);
            if module_satisfies || item_satisfies {
                satisfiers.push(m.name.clone());
            }
        }
        satisfiers.sort();
        satisfiers.dedup();
        if satisfiers.is_empty() {
            orphans.push(req.clone());
        }
        matrix.push(CoverageRow {
            requirement: req.clone(),
            satisfied_by: satisfiers,
        });
    }

    let retired = crate::retired::retired_ids(&EntryStore::for_layout(layout))
        .into_iter()
        .collect();
    Ok(CoverageReport {
        matrix,
        orphans,
        retired,
    })
}

/// Render to YAML — the default format. Stable key ordering across runs.
pub fn render_yaml(report: &CoverageReport) -> Result<String> {
    serde_yaml::to_string(report).context("serializing coverage report to YAML")
}

/// Render to Markdown — human-readable opt-in via `--format markdown`.
pub fn render_markdown(report: &CoverageReport) -> String {
    let mut out = String::new();
    out.push_str("# Coverage matrix\n\n");
    if report.matrix.is_empty() {
        out.push_str("_No requirements found: no `[[requirement]]` entries are authored._\n");
        return out;
    }
    out.push_str("| Requirement | Satisfied by |\n");
    out.push_str("|---|---|\n");
    for row in &report.matrix {
        let cell = if row.satisfied_by.is_empty() {
            "**ORPHAN** — no module satisfies this requirement.".to_string()
        } else {
            row.satisfied_by
                .iter()
                .map(|m| format!("`{m}`"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("| `{}` | {cell} |\n", row.requirement));
    }
    if !report.orphans.is_empty() {
        out.push_str("\n## Orphans\n\n");
        for o in &report.orphans {
            out.push_str(&format!("- `{o}`\n"));
        }
    }
    if !report.retired.is_empty() {
        out.push_str("\n## Retired\n\n");
        for r in &report.retired {
            out.push_str(&format!("- `{r}`\n"));
        }
    }
    out
}

/// Module-level view used by coverage construction. Keeps the type
/// local to `coverage.rs` rather than borrowing `skeleton.rs`'s
/// `ModuleSpec` (which carried emission-only fields).
struct ModuleView {
    name: String,
    satisfies: Vec<String>,
    /// Flattened satisfies across all items[] entries. Coverage doesn't
    /// care which item satisfies a requirement, only that the module
    /// does.
    item_satisfies: Vec<String>,
}

fn load_modules(layout: &LayoutContext) -> Result<Vec<ModuleView>> {
    let mut module_paths: Vec<std::path::PathBuf> = Vec::new();
    if layout.modules_dir.is_dir() {
        for entry in std::fs::read_dir(&layout.modules_dir)
            .with_context(|| format!("reading {}", layout.modules_dir.display()))?
        {
            let entry = entry?;
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("toml") {
                module_paths.push(p);
            }
        }
        module_paths.sort();
    }

    // Loaded once; only required for modules that actually declare items[]
    // (parse_spec_items surfaces the load error per-module in that case).
    let kind_vocab = crate::items::load_item_kinds(layout);
    let mut modules: Vec<ModuleView> = Vec::with_capacity(module_paths.len());
    for path in &module_paths {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let value: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let module_name = value
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "[COVERAGE-002] module spec at {} missing required `name` field",
                    path.display()
                )
            })?
            .to_string();
        let label = path.display().to_string();
        let items = parse_spec_items(&value, &label, &kind_vocab)?;
        let satisfies = value
            .get("satisfies")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let mut item_satisfies: Vec<String> = items
            .iter()
            .flat_map(|i| i.satisfies.iter().cloned())
            .collect();
        item_satisfies.sort();
        item_satisfies.dedup();
        modules.push(ModuleView {
            name: module_name,
            satisfies,
            item_satisfies,
        });
    }
    Ok(modules)
}

/// Read `[[requirement]] id` entries from `specs/requirements.toml`.
/// Returns an empty list (not an error) when the file is missing — the
/// coverage report presents an empty matrix in that case rather than
/// refusing to run.
///
/// The array key is `requirement` (singular) per the
/// `requirements.schema.yaml` contract across all profiles. Carried
/// over verbatim from the retired `skeleton::load_requirement_ids`.
fn load_requirement_ids(layout: &LayoutContext) -> Result<Vec<String>> {
    let arr: Vec<toml::Value> = EntryStore::for_layout(layout)
        .load(EntryKind::Requirement)?
        .into_iter()
        .filter(|e| !crate::entries::is_retired_file(&e.file))
        .map(|e| e.value)
        .collect();
    // Preserve declaration order; dedup retains the first occurrence
    // (a duplicate id is a separate validate-time error caught upstream).
    let mut ids: Vec<String> = Vec::with_capacity(arr.len());
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    for t in &arr {
        if let Some(id) = t.get("id").and_then(|v| v.as_str()) {
            if seen.insert(id.to_string(), ()).is_none() {
                ids.push(id.to_string());
            }
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_parse_yaml_default() {
        assert!(matches!(
            CoverageFormat::parse("yaml").unwrap(),
            CoverageFormat::Yaml
        ));
    }

    #[test]
    fn format_parse_markdown() {
        assert!(matches!(
            CoverageFormat::parse("markdown").unwrap(),
            CoverageFormat::Markdown
        ));
        assert!(matches!(
            CoverageFormat::parse("md").unwrap(),
            CoverageFormat::Markdown
        ));
    }

    #[test]
    fn format_parse_unknown_errors_coverage_001() {
        let err = CoverageFormat::parse("html").unwrap_err();
        assert!(format!("{err:#}").contains("COVERAGE-001"));
    }

    #[test]
    fn render_markdown_empty_says_no_requirements() {
        let report = CoverageReport {
            matrix: Vec::new(),
            orphans: Vec::new(),
            retired: vec![],
        };
        let md = render_markdown(&report);
        assert!(md.contains("No requirements found"));
    }

    #[test]
    fn render_markdown_orphan_flagged_in_table() {
        let report = CoverageReport {
            matrix: vec![CoverageRow {
                requirement: "REQ-001".into(),
                satisfied_by: Vec::new(),
            }],
            orphans: vec!["REQ-001".into()],
            retired: vec![],
        };
        let md = render_markdown(&report);
        assert!(md.contains("ORPHAN"));
        assert!(md.contains("REQ-001"));
    }

    #[test]
    fn render_yaml_round_trips_via_serde() {
        let report = CoverageReport {
            matrix: vec![CoverageRow {
                requirement: "REQ-001".into(),
                satisfied_by: vec!["alpha".into(), "beta".into()],
            }],
            orphans: Vec::new(),
            retired: vec![],
        };
        let y = render_yaml(&report).unwrap();
        assert!(y.contains("REQ-001"));
        assert!(y.contains("alpha"));
        assert!(y.contains("beta"));
    }
}
