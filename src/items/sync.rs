//! Phase 14 three-stage sync — Stage 1: deterministic detection
//! (`items diff`).
//!
//! Walks a module's spec items[] and the corresponding source file's
//! tree-sitter-extracted items, joins them on `(kind, name)`, and emits
//! a structured drift report. No AI involvement at this stage; the
//! output is the substrate that `items propose` (14-F) wraps in an
//! LLM-advisory pass.
//!
//! Drift categories:
//!
//! - `in_spec_only`  — spec carries the item but the source does not
//!   (deleted in code without spec update, OR pre-implementation).
//! - `in_source_only`— source carries the item but the spec does not
//!   (helper/contractual ambiguity — `propose` will advise).
//! - `both` — present on both sides; if the spec carries
//!   `signature_hint` and it diverges from the source's tree-sitter
//!   signature, the pair flags `signature_drift`.
//!
//! The diff is intentionally syntactic — items are matched by kind +
//! name. Semantic drift (a function whose body changed but signature
//! didn't) is out of scope for `items diff`; that's the job of
//! `verified_by_resolves` and downstream test runs.
//!
//! Error codes: `ITEM-101` (module spec missing), `ITEM-102`
//! (module spec parse fails), `ITEM-103` (serialization).

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::path::PathBuf;

use crate::items::{extract, source_map, ExtractedItem, ItemKind};
use crate::layout::LayoutContext;

/// One spec-side item as parsed from `[[items]]` TOML. Mirrors the
/// language schema's required + optional fields with kind-discriminated
/// extras preserved as raw TOML so `propose`/`apply` can round-trip
/// them without loss.
#[derive(Debug, Clone, Serialize)]
pub struct SpecItem {
    pub kind: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub satisfies: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_hint: Option<String>,
    /// Kind-discriminated extras (target/visibility/alias for
    /// re_exports; target_type/trait_ref for impls). Carried verbatim.
    #[serde(skip_serializing_if = "serde_json::Map::is_empty", default)]
    pub extras: serde_json::Map<String, serde_json::Value>,
}

/// Pair of spec + source views of the same (kind, name) item.
#[derive(Debug, Serialize)]
pub struct DiffPair {
    pub spec: SpecItem,
    pub source: ExtractedItem,
    /// `Some(reason)` when the spec carries `signature_hint` and the
    /// source's tree-sitter signature diverges. `None` otherwise — both
    /// "no hint" and "hint matches" produce `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature_drift: Option<String>,
}

/// Top-level diff output for one module.
#[derive(Debug, Serialize)]
pub struct DiffReport {
    pub module: String,
    pub source_file: Option<PathBuf>,
    pub in_spec_only: Vec<SpecItem>,
    pub in_source_only: Vec<ExtractedItem>,
    pub both: Vec<DiffPair>,
}

impl DiffReport {
    /// `true` when there is no drift between spec and source.
    pub fn is_clean(&self) -> bool {
        self.in_spec_only.is_empty()
            && self.in_source_only.is_empty()
            && self.both.iter().all(|p| p.signature_drift.is_none())
    }
}

/// Parse `module.kind` enum from its schema-facing string form.
fn parse_kind(s: &str) -> Option<ItemKind> {
    Some(match s {
        "fn" => ItemKind::Fn,
        "struct" => ItemKind::Struct,
        "enum" => ItemKind::Enum,
        "union" => ItemKind::Union,
        "trait" => ItemKind::Trait,
        "impl" => ItemKind::Impl,
        "type" => ItemKind::Type,
        "const" => ItemKind::Const,
        "static" => ItemKind::Static,
        "re_export" => ItemKind::ReExport,
        "macro" => ItemKind::Macro,
        "extern_block" => ItemKind::ExternBlock,
        _ => return None,
    })
}

/// Walk a module spec's `items` array and produce typed `SpecItem`
/// entries. Unknown kinds become a parse error (`ITEM-102`).
fn parse_spec_items(value: &toml::Value, module_label: &str) -> Result<Vec<SpecItem>> {
    let items = match value.get("items") {
        Some(toml::Value::Array(arr)) => arr,
        Some(other) => {
            return Err(anyhow!(
                "[ITEM-102] {module_label}: `items` must be an array of tables; got {}",
                other.type_str()
            ))
        }
        None => return Ok(Vec::new()),
    };

    let mut out = Vec::with_capacity(items.len());
    for (i, raw) in items.iter().enumerate() {
        let table = raw.as_table().ok_or_else(|| {
            anyhow!(
                "[ITEM-102] {module_label}: items[{i}] must be a table; got {}",
                raw.type_str()
            )
        })?;
        let kind = table
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow!("[ITEM-102] {module_label}: items[{i}] missing required `kind`")
            })?
            .to_string();
        if parse_kind(&kind).is_none() {
            return Err(anyhow!(
                "[ITEM-102] {module_label}: items[{i}] unknown kind {kind:?}; \
                 expected one of fn/struct/enum/union/trait/impl/type/const/static/\
                 re_export/macro/extern_block"
            ));
        }
        let name = table
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow!("[ITEM-102] {module_label}: items[{i}] missing required `name`")
            })?
            .to_string();
        let description = table
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let satisfies = table
            .get("satisfies")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let signature_hint = table
            .get("signature_hint")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // Capture kind-discriminated extras. Convert the table to JSON
        // and strip the canonical keys; whatever remains rides through
        // verbatim.
        let mut extras = toml_table_to_json_map(table);
        for k in [
            "kind",
            "name",
            "description",
            "satisfies",
            "signature_hint",
        ] {
            extras.remove(k);
        }

        out.push(SpecItem {
            kind,
            name,
            description,
            satisfies,
            signature_hint,
            extras,
        });
    }

    Ok(out)
}

/// Best-effort `toml::Value::Table` → `serde_json::Map` conversion.
/// Preserves nested arrays/tables; loses the original TOML formatting
/// (callers that need round-trip fidelity should re-read the file).
fn toml_table_to_json_map(t: &toml::value::Table) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for (k, v) in t {
        out.insert(k.clone(), toml_to_json(v));
    }
    out
}

fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(d) => serde_json::Value::String(d.to_string()),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(t) => serde_json::Value::Object(toml_table_to_json_map(t)),
    }
}

/// Compare a spec `signature_hint` against an extracted item's
/// signature. Whitespace-normalized, otherwise-strict. Returns
/// `Some(reason)` when they disagree.
fn signature_drift(hint: &str, source: &ExtractedItem) -> Option<String> {
    let extracted = source.signature.as_deref().unwrap_or("");
    let normalize = |s: &str| {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    if normalize(hint) == normalize(extracted) {
        None
    } else {
        Some(format!(
            "spec.signature_hint = {hint:?}; source signature = {extracted:?}"
        ))
    }
}

/// Run a deterministic diff for `module_name`. The module's spec must
/// exist under `specs/modules/<module_name>.toml`; missing source files
/// are treated as `in_spec_only` for every spec item (pre-implementation
/// modules emit clean drift on the source side, never error).
pub fn diff_module(layout: &LayoutContext, module_name: &str) -> Result<DiffReport> {
    let module_path = layout.modules_dir.join(format!("{module_name}.toml"));
    if !module_path.is_file() {
        return Err(anyhow!(
            "[ITEM-101] module spec not found at {}",
            module_path.display()
        ));
    }
    let label = module_path.display().to_string();
    let toml_text = std::fs::read_to_string(&module_path)
        .with_context(|| format!("reading {}", module_path.display()))?;
    let value: toml::Value = toml::from_str(&toml_text)
        .with_context(|| format!("[ITEM-102] {label}: TOML parse failed"))?;
    let spec_items = parse_spec_items(&value, &label)?;

    // Resolve source file via the language adapter (Phase 10 surface).
    let language = &layout.spec.project.language;
    let adapter = crate::lang::adapter_for(language)
        .ok_or_else(|| anyhow!("[ITEM-101] no language adapter for `{language}`"))?;
    let source_root = layout.spec.paths.source.as_str();
    let source_file = source_map::resolve_source_path(
        module_name,
        adapter.as_ref(),
        &layout.project_root,
        source_root,
    );

    let extracted: Vec<ExtractedItem> = match &source_file {
        Some(p) => extract::extract_items_from_path(p, adapter.as_ref())
            .with_context(|| format!("extracting items from {}", p.display()))?,
        None => Vec::new(),
    };

    // Index source items by (kind, name) for O(1) lookup as we walk
    // the spec side. Use a Vec for stable ordering when emitting the
    // in_source_only bucket.
    let mut source_remaining: Vec<ExtractedItem> = extracted;

    let mut in_spec_only: Vec<SpecItem> = Vec::new();
    let mut both: Vec<DiffPair> = Vec::new();

    for spec in spec_items {
        let kind = parse_kind(&spec.kind).expect("validated in parse_spec_items");
        let pos = source_remaining
            .iter()
            .position(|s| s.kind == kind && s.name == spec.name);
        match pos {
            Some(idx) => {
                let source = source_remaining.remove(idx);
                let drift = spec
                    .signature_hint
                    .as_deref()
                    .and_then(|h| signature_drift(h, &source));
                both.push(DiffPair {
                    spec,
                    source,
                    signature_drift: drift,
                });
            }
            None => in_spec_only.push(spec),
        }
    }

    Ok(DiffReport {
        module: module_name.to_string(),
        source_file,
        in_spec_only,
        in_source_only: source_remaining,
        both,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> crate::items::Span {
        crate::items::Span {
            start_line: 1,
            end_line: 1,
        }
    }

    #[test]
    fn signature_drift_normalizes_whitespace() {
        let item = ExtractedItem {
            kind: ItemKind::Fn,
            name: "foo".into(),
            signature: Some("fn  foo(x:  u32)  ->  u32".into()),
            attributes: vec![],
            span: span(),
        };
        assert!(signature_drift("fn foo(x: u32) -> u32", &item).is_none());
        assert!(signature_drift("fn foo(x: u64) -> u32", &item).is_some());
    }

    #[test]
    fn signature_drift_empty_extracted_signature_treats_as_drift() {
        let item = ExtractedItem {
            kind: ItemKind::Fn,
            name: "foo".into(),
            signature: None,
            attributes: vec![],
            span: span(),
        };
        // Hint vs. nothing → drift.
        let d = signature_drift("fn foo()", &item);
        assert!(d.is_some(), "expected drift; got {d:?}");
    }

    #[test]
    fn parse_spec_items_round_trip_preserves_extras() {
        let toml_text = r#"
[[items]]
name = "Reexp"
kind = "re_export"
description = "alias"
target = "inner::T"
visibility = "pub"
"#;
        let v: toml::Value = toml::from_str(toml_text).unwrap();
        let items = parse_spec_items(&v, "test").unwrap();
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.kind, "re_export");
        assert_eq!(it.name, "Reexp");
        assert!(it.extras.contains_key("target"));
        assert!(it.extras.contains_key("visibility"));
        assert!(!it.extras.contains_key("kind"));
    }

    #[test]
    fn parse_spec_items_rejects_unknown_kind() {
        let toml_text = r#"
[[items]]
name = "Foo"
kind = "lambda"
description = "x"
"#;
        let v: toml::Value = toml::from_str(toml_text).unwrap();
        let err = parse_spec_items(&v, "test").unwrap_err();
        assert!(format!("{err:#}").contains("ITEM-102"));
        assert!(format!("{err:#}").contains("lambda"));
    }
}
