//! Three-stage sync — Stage 2 scaffolding: `items propose`.
//!
//! Builds a *proposal scaffold* from `items diff` output. The scaffold
//! is purely deterministic: every drift entry is enriched with the
//! relevant source snippet (sliced by tree-sitter span) and the
//! corresponding spec [[items]] mapping, then written to
//! `specs/specdev/upstream/items-sync/<module>.proposal.yaml` for the
//! `/spec-sync-items` skill (and Claude inside it) to fill in.
//!
//! What this module deliberately does NOT do:
//!
//! - Call any LLM. The "advisory" half of Stage 2 lives in the skill
//!   that wraps this CLI; the analyzer keeps determinism so it can run
//!   in CI / pre-commit and produce an identical scaffold each time.
//! - Decide what action to take. Every entry's `recommendation.action`
//!   ships as `"pending"`; the skill (or a human) sets it to
//!   `add-to-spec` / `remove-from-spec` / `rename` / `skip`.
//! - Mutate `specs/modules/<Name>.toml`. That's `items apply` (14-G).
//!
//! Re-running `items propose` overwrites the existing proposal file
//! unless `--no-clobber` is passed. Preserving rationales across
//! re-runs lives behind the explicit flag because the failure mode
//! "code drifted further; old rationales now apply to wrong shape" is
//! quieter than "I lost my unsubmitted rationales" — we want the loud
//! one to be the default.
//!
//! Error codes: `ITEM-201` (proposal write failure), `ITEM-202`
//! (existing proposal + no `--clobber`), `ITEM-203` (source read).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use crate::items::{sync, Span};
use crate::layout::LayoutContext;

/// Top-level proposal document persisted to
/// `<specdev_root>/upstream/items-sync/<module>.proposal.yaml`.
///
/// `schema_version: 1` lets future propose/apply changes evolve the
/// shape without silently misreading older proposals.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProposalDocument {
    pub schema_version: u32,
    pub module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<PathBuf>,
    pub generated_at: String,
    /// Lifecycle: `no-drift` | `needs-rationale` | `awaiting-approval`
    /// | `approved` | `applied`. The skill flips
    /// `needs-rationale` → `awaiting-approval` after Claude fills in
    /// each entry's recommendation; the human flips
    /// `awaiting-approval` → `approved` by setting `approved: true`
    /// + `approver` + `approved_at`; `items apply` flips
    ///   `approved` → `applied` and timestamps the audit log entry.
    pub status: String,
    #[serde(default)]
    pub approved: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_at: Option<String>,
    pub entries: Vec<ProposalEntry>,
}

/// One drift entry awaiting recommendation.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProposalEntry {
    /// Stable id within the proposal: `spec-only-<n>` /
    /// `source-only-<n>`. Used by `items apply` to look
    /// up the entry and by skill prose for cross-reference.
    pub id: String,
    /// `in_spec_only` | `in_source_only`.
    pub category: String,
    pub kind: String,
    pub name: String,
    /// Raw source-file slice (1-based span lines, joined with `\n`).
    /// Populated for `in_source_only` entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_snippet: Option<String>,
    /// Source span (1-based, inclusive) for `in_source_only`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    /// Spec-side [[items]] entry as a YAML mapping. Populated for
    /// `in_spec_only`. Carries only the typed
    /// fields the spec author wrote — kind-discriminated extras
    /// (target/visibility for re_exports, target_type/trait_ref for
    /// impls) ride through under `extras`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_item: Option<serde_yaml::Value>,
    pub recommendation: Recommendation,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Recommendation {
    /// `pending` (initial) | `skip` | `add-to-spec` | `remove-from-spec`
    /// | `rename`. `items apply` rejects any entry still
    /// on `pending` when `approved: true`.
    pub action: String,
    /// Free-form prose from the skill / human. Empty on initial scaffold.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rationale: String,
    /// For `add-to-spec`: the [[items]] table the
    /// skill / human wants written into the spec. Required keys:
    /// `kind` + `name`; recommended: `description` + `satisfies`.
    /// Carried as a YAML mapping so the proposal file is hand-editable
    /// without round-tripping TOML inside YAML inside TOML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_spec: Option<serde_yaml::Value>,
    /// For `rename` only: the new name to write onto the existing spec
    /// item identified by this entry's `(kind, name)`. Every other field
    /// of the matched item — `satisfies`, `verifies`, `description`,
    /// kind-discriminated extras — is preserved untouched, which is the
    /// whole point: a rename keeps the requirement/verification trace
    /// that a remove + add would force the author to re-author by hand.
    /// Set this on the `in_spec_only` entry carrying the old name; mark
    /// the paired `in_source_only` entry (the new name) `skip` — the
    /// rename makes the next diff pair them with no further action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rename_to: Option<String>,
}

/// Default on-disk path for a module's proposal file.
pub fn proposal_path(layout: &LayoutContext, module_name: &str) -> PathBuf {
    layout
        .specdev_root
        .join("upstream")
        .join("items-sync")
        .join(format!("{module_name}.proposal.yaml"))
}

/// Build (but do not persist) a proposal document for `module_name`.
/// Returns Err on hard failures (missing module spec, parse error,
/// source-read error). An empty drift set still returns Ok with
/// `status: no-drift` and an empty `entries` vec.
pub fn build_proposal(layout: &LayoutContext, module_name: &str) -> Result<ProposalDocument> {
    let report = sync::diff_module(layout, module_name)?;

    // Read source once; cache lines for span slicing.
    let source_text = match report
        .source_file
        .as_ref()
        .map(|p| layout.code_root.join(p))
    {
        Some(p) => Some(
            std::fs::read_to_string(&p)
                .with_context(|| format!("[ITEM-203] reading source {}", p.display()))?,
        ),
        None => None,
    };
    let source_lines: Vec<&str> = source_text
        .as_deref()
        .map(|s| s.lines().collect())
        .unwrap_or_default();

    let mut entries: Vec<ProposalEntry> = Vec::new();

    for (i, spec) in report.in_spec_only.iter().enumerate() {
        entries.push(ProposalEntry {
            id: format!("spec-only-{i}"),
            category: "in_spec_only".into(),
            kind: spec.kind.clone(),
            name: spec.name.clone(),
            source_snippet: None,
            span: None,
            spec_item: Some(spec_item_to_yaml(spec)),
            recommendation: Recommendation {
                action: "pending".into(),
                rationale: String::new(),
                proposed_spec: None,
                rename_to: None,
            },
        });
    }

    for (i, src) in report.in_source_only.iter().enumerate() {
        entries.push(ProposalEntry {
            id: format!("source-only-{i}"),
            category: "in_source_only".into(),
            kind: src.kind.clone(),
            name: src.name.clone(),
            source_snippet: slice_snippet(&source_lines, src.span),
            span: Some(src.span),
            spec_item: None,
            recommendation: Recommendation {
                action: "pending".into(),
                rationale: String::new(),
                proposed_spec: None,
                rename_to: None,
            },
        });
    }

    let status = if entries.is_empty() {
        "no-drift".to_string()
    } else {
        "needs-rationale".to_string()
    };

    Ok(ProposalDocument {
        schema_version: 1,
        module: module_name.to_string(),
        source_file: report.source_file,
        generated_at: chrono::Utc::now().to_rfc3339(),
        status,
        approved: false,
        approver: None,
        approved_at: None,
        entries,
    })
}

/// Persist `doc` to `path`. Refuses to clobber an existing file unless
/// `clobber` is true. Creates parent directories as needed.
pub fn write_proposal(doc: &ProposalDocument, path: &Path, clobber: bool) -> Result<()> {
    if path.exists() && !clobber {
        return Err(anyhow!(
            "[ITEM-202] proposal already exists at {}; pass --clobber to overwrite \
             (rationales in the existing file will be lost — review and apply first \
             if you want to keep them)",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "[ITEM-201] creating proposal directory {}",
                parent.display()
            )
        })?;
    }
    let yaml = serde_yaml::to_string(doc)
        .with_context(|| format!("[ITEM-201] serializing proposal for {}", doc.module))?;
    std::fs::write(path, yaml)
        .with_context(|| format!("[ITEM-201] writing proposal to {}", path.display()))?;
    Ok(())
}

fn slice_snippet(lines: &[&str], span: Span) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let start = span.start_line.saturating_sub(1);
    let end = span.end_line.min(lines.len());
    if start >= end {
        return None;
    }
    Some(lines[start..end].join("\n"))
}

/// Convert a `SpecItem` to a YAML mapping shaped like the spec's
/// `[[items]]` table. Kind-discriminated extras ride through as
/// nested fields under their original keys (target, visibility, etc.).
fn spec_item_to_yaml(spec: &sync::SpecItem) -> serde_yaml::Value {
    let mut map = serde_yaml::Mapping::new();
    map.insert("kind".into(), spec.kind.clone().into());
    map.insert("name".into(), spec.name.clone().into());
    if !spec.description.is_empty() {
        map.insert("description".into(), spec.description.clone().into());
    }
    if !spec.satisfies.is_empty() {
        let arr: Vec<serde_yaml::Value> = spec
            .satisfies
            .iter()
            .map(|s| serde_yaml::Value::String(s.clone()))
            .collect();
        map.insert("satisfies".into(), serde_yaml::Value::Sequence(arr));
    }
    for (k, v) in &spec.extras {
        let yaml_v = json_to_yaml(v.clone());
        map.insert(serde_yaml::Value::String(k.clone()), yaml_v);
    }
    serde_yaml::Value::Mapping(map)
}

fn json_to_yaml(v: serde_json::Value) -> serde_yaml::Value {
    match v {
        serde_json::Value::Null => serde_yaml::Value::Null,
        serde_json::Value::Bool(b) => serde_yaml::Value::Bool(b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                serde_yaml::Value::Number(i.into())
            } else if let Some(f) = n.as_f64() {
                serde_yaml::Value::Number(f.into())
            } else {
                serde_yaml::Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => serde_yaml::Value::String(s),
        serde_json::Value::Array(arr) => {
            serde_yaml::Value::Sequence(arr.into_iter().map(json_to_yaml).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut m = serde_yaml::Mapping::new();
            for (k, v) in obj {
                m.insert(serde_yaml::Value::String(k), json_to_yaml(v));
            }
            serde_yaml::Value::Mapping(m)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_snippet_returns_inclusive_lines() {
        let text = "a\nb\nc\nd\ne";
        let lines: Vec<&str> = text.lines().collect();
        let span = Span {
            start_line: 2,
            end_line: 4,
        };
        assert_eq!(slice_snippet(&lines, span).as_deref(), Some("b\nc\nd"));
    }

    #[test]
    fn slice_snippet_clamps_end_past_eof() {
        let text = "a\nb";
        let lines: Vec<&str> = text.lines().collect();
        let span = Span {
            start_line: 2,
            end_line: 99,
        };
        assert_eq!(slice_snippet(&lines, span).as_deref(), Some("b"));
    }

    #[test]
    fn slice_snippet_empty_when_no_lines() {
        let span = Span {
            start_line: 1,
            end_line: 1,
        };
        assert!(slice_snippet(&[], span).is_none());
    }

    #[test]
    fn spec_item_to_yaml_round_trips_extras() {
        let mut extras = serde_json::Map::new();
        extras.insert(
            "target".into(),
            serde_json::Value::String("inner::T".into()),
        );
        extras.insert("visibility".into(), serde_json::Value::String("pub".into()));
        let spec = sync::SpecItem {
            kind: "re_export".into(),
            name: "Reexp".into(),
            description: "alias".into(),
            satisfies: vec![],
            extras,
        };
        let y = spec_item_to_yaml(&spec);
        let m = y.as_mapping().expect("mapping");
        assert_eq!(
            m.get(serde_yaml::Value::String("kind".into()))
                .and_then(|v| v.as_str()),
            Some("re_export")
        );
        assert_eq!(
            m.get(serde_yaml::Value::String("target".into()))
                .and_then(|v| v.as_str()),
            Some("inner::T")
        );
        assert_eq!(
            m.get(serde_yaml::Value::String("visibility".into()))
                .and_then(|v| v.as_str()),
            Some("pub")
        );
    }
}
