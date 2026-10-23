//! Three-stage sync — Stage 1: deterministic detection
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
//! - `both` — present on both sides.
//!
//! The diff is intentionally syntactic — items are matched by kind +
//! name. An item carries no signature, so signature and behaviour
//! changes are out of scope for `items diff`; that's the job of
//! `verified_by_resolves` and downstream test runs.
//!
//! Error codes: `ITEM-101` (module spec missing), `ITEM-102`
//! (module spec parse fails), `ITEM-103` (serialization), `ITEM-104`
//! (language tier's item-kind vocabulary not inlined while the module
//! declares items).

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use std::path::PathBuf;

use crate::items::{extract, source_map, ExtractedItem};
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
}

/// Top-level diff output for one module.
#[derive(Debug, Serialize)]
pub struct DiffReport {
    pub module: String,
    pub source_file: Option<PathBuf>,
    pub in_spec_only: Vec<SpecItem>,
    pub in_source_only: Vec<ExtractedItem>,
    pub both: Vec<DiffPair>,
    /// Set when no source file resolves while the spec lists items, so
    /// every item reads `in_spec_only` for that reason alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl DiffReport {
    /// `true` when there is no drift between spec and source.
    pub fn is_clean(&self) -> bool {
        self.in_spec_only.is_empty() && self.in_source_only.is_empty()
    }
}

/// Normalize an impl identity: strip generic argument lists (F-010), then
/// reduce each path-qualified trait/type to its final `::` segment (F-013).
/// `core::fmt::Display for RateLimiter<C>` → `Display for RateLimiter`;
/// `From<Vec<u8>> for crate::Foo` → `From for Foo`. The ` for ` separator only
/// appears in trait impls; inherent impls (`RateLimiter`) have a single path.
///
/// This is Rust identity vocabulary — the Rust adapter's
/// `item_identity_key` applies it to `impl` items; the diff join itself is
/// language-neutral and takes its keys from the adapter.
pub(crate) fn normalize_impl_name(name: &str) -> String {
    let degeneric = strip_generics(name);
    match degeneric.split_once(" for ") {
        Some((trait_ref, target)) => {
            format!("{} for {}", last_segment(trait_ref), last_segment(target))
        }
        None => last_segment(&degeneric).to_string(),
    }
}

/// Last `::`-delimited segment of a path, trimmed. `core::fmt::Display` →
/// `Display`; `RateLimited` → `RateLimited`. Two distinct paths sharing a final
/// segment (`a::Display` vs `b::Display`) collapse together — an accepted edge,
/// the same class as F-010's distinct generic instantiations.
fn last_segment(path: &str) -> &str {
    path.trim().rsplit("::").next().unwrap_or(path).trim()
}

/// Canonical base of a type expression: generics stripped (F-010), then the
/// final `::` segment. `RateLimiter<C>` → `RateLimiter`; `crate::Foo` → `Foo`.
/// Used by extraction to build inherent-method qualified names
/// (`<base>::<m>`) so the emitted name matches what an author writes — `Fn`
/// identity keys are compared verbatim (no normalize step), unlike `impl` keys.
pub(crate) fn type_base(type_expr: &str) -> String {
    last_segment(&strip_generics(type_expr)).to_string()
}

/// Drop every balanced `<…>` group (and the angle brackets themselves) from an
/// impl identity, collapsing the whitespace left behind. Handles trait impls
/// and nested generics: `Clock for RateLimiter<C>` → `Clock for RateLimiter`,
/// `From<Vec<u8>> for Foo` → `From for Foo`. Impl headers are the only place a
/// `<` appears in a synthesized name, so a depth counter is sufficient.
fn strip_generics(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth: usize = 0;
    for ch in name.chars() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Walk a module spec's `items` array and produce typed `SpecItem`
/// entries. Unknown kinds become a parse error (`ITEM-102`), checked
/// against the language tier's runtime-loaded kind vocabulary
/// (`kind_vocab`, from [`super::load_item_kinds`]). The vocabulary is
/// only *required* when the module actually declares items — a project
/// whose language ships no items tier parses cleanly as long as its
/// modules carry no `[[items]]` (`ITEM-104` otherwise).
pub(crate) fn parse_spec_items(
    value: &toml::Value,
    module_label: &str,
    kind_vocab: &Result<Vec<String>>,
) -> Result<Vec<SpecItem>> {
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
        let allowed = kind_vocab
            .as_ref()
            .map_err(|e| anyhow!("[ITEM-104] {module_label}: {e}"))?;
        if !allowed.iter().any(|k| k == &kind) {
            return Err(anyhow!(
                "[ITEM-102] {module_label}: items[{i}] unknown kind {kind:?}; \
                 expected one of {}",
                allowed.join("/")
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
        // Capture kind-discriminated extras. Convert the table to JSON
        // and strip the canonical keys; whatever remains rides through
        // verbatim.
        let mut extras = toml_table_to_json_map(table);
        for k in ["kind", "name", "description", "satisfies"] {
            extras.remove(k);
        }

        out.push(SpecItem {
            kind,
            name,
            description,
            satisfies,
            extras,
        });
    }

    Ok(out)
}

/// True when a spec item is marked `test = true`. Test items ride the
/// `test` flag through `extras` (it's not one of the canonical keys), so
/// the boolean is read from there. Used to exclude tests from the
/// public-surface diff (R-001).
fn is_test_item(item: &SpecItem) -> bool {
    item.extras
        .get("test")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
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
        toml::Value::Array(arr) => serde_json::Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => serde_json::Value::Object(toml_table_to_json_map(t)),
    }
}

/// A spec item named `<Trait>::<member>` whose `<Trait>` is a matched
/// trait item: the members of a trait declaration are not items (the
/// trait is one item, as a trait impl is), so the spec item stays
/// `in_spec_only`. Say so once, naming the items.
fn trait_member_note(in_spec_only: &[SpecItem], both: &[DiffPair]) -> Option<String> {
    let traits: Vec<&str> = both
        .iter()
        .filter(|p| p.source.kind == "trait")
        .map(|p| p.source.name.as_str())
        .collect();
    let members: Vec<&str> = in_spec_only
        .iter()
        .filter(|i| {
            i.name
                .split_once("::")
                .is_some_and(|(host, _)| traits.contains(&host))
        })
        .map(|i| i.name.as_str())
        .collect();
    (!members.is_empty()).then(|| {
        format!(
            "{} {} a member of a trait declaration; a trait is one item, so describe its \
             members in the trait's `description` and remove these items",
            members.join(", "),
            if members.len() == 1 { "is" } else { "are each" }
        )
    })
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
    // Test items (`test = true`) are out of scope for public-surface
    // comparison (R-001). They're verified through `verifies` + `cargo test`,
    // not tree-sitter extraction — and the substrate can't see them anyway
    // (unit tests live in nested `#[cfg(test)] mod tests`; integration tests
    // live in `tests/<name>.rs`, not the module's own source_file). Excluding
    // them here keeps test-bearing modules from reading as permanent
    // `in_spec_only` drift, aligning the diff with the items-model
    // verification-graph boundary.
    let kind_vocab = super::load_item_kinds(layout);
    let spec_items: Vec<SpecItem> = parse_spec_items(&value, &label, &kind_vocab)?
        .into_iter()
        .filter(|item| !is_test_item(item))
        .collect();

    // Resolve source file via the language adapter.
    let language = &layout.spec.project.language;
    let adapter = crate::lang::adapter_for(language)
        .ok_or_else(|| anyhow!("[ITEM-101] no language adapter for `{language}`"))?;
    let source_root = layout.spec.paths.source.as_str();
    // #101(a): the spec's `source_file` override; #101(b): the profile floor.
    let source_file = source_map::resolve_source_path(
        module_name,
        source_map::source_file_override(&value),
        adapter.as_ref(),
        &layout.code_root,
        source_root,
    );
    let floor = extract::profile_visibility_floor(&layout.specdev_root)?;

    let extracted: Vec<ExtractedItem> = match &source_file {
        Some(p) => extract::extract_items_from_path(p, adapter.as_ref(), floor.as_deref())
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
        let spec_key = adapter.item_identity_key(&spec.kind, &spec.name);
        let pos = source_remaining.iter().position(|s| {
            s.kind == spec.kind && adapter.item_identity_key(&s.kind, &s.name) == spec_key
        });
        match pos {
            Some(idx) => {
                let source = source_remaining.remove(idx);
                both.push(DiffPair { spec, source });
            }
            None => in_spec_only.push(spec),
        }
    }

    let note = if source_file.is_none() && !in_spec_only.is_empty() {
        Some(
            "no source file resolves for this module, so every item is `in_spec_only`; \
             `specdev-analyzer check` names a candidate file, or set `source_file`"
                .to_string(),
        )
    } else {
        trait_member_note(&in_spec_only, &both)
    };
    Ok(DiffReport {
        module: module_name.to_string(),
        source_file: source_file.map(|p| super::util::relative_to(&layout.code_root, &p)),
        in_spec_only,
        in_source_only: source_remaining,
        both,
        note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rust's 12-kind vocabulary, as the runtime loader would produce it
    /// from the language tier — unit tests here exercise the parse logic
    /// without a bootstrapped project on disk.
    fn rust_kinds() -> Result<Vec<String>> {
        Ok([
            "fn",
            "struct",
            "enum",
            "union",
            "trait",
            "impl",
            "type",
            "const",
            "static",
            "re_export",
            "macro",
            "extern_block",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect())
    }

    /// The Rust adapter's identity key — shorthand for the join-key tests.
    fn identity_key(kind: &str, name: &str) -> String {
        use crate::lang::LanguageAdapter;
        crate::lang::rust::RustAdapter.item_identity_key(kind, name)
    }

    #[test]
    fn strip_generics_removes_balanced_groups() {
        assert_eq!(strip_generics("RateLimiter<C>"), "RateLimiter");
        assert_eq!(strip_generics("RateLimiter"), "RateLimiter");
        assert_eq!(
            strip_generics("Clock for RateLimiter<C>"),
            "Clock for RateLimiter"
        );
        assert_eq!(strip_generics("From<u32> for Foo"), "From for Foo");
        assert_eq!(strip_generics("From<Vec<u8>> for Foo"), "From for Foo");
    }

    #[test]
    fn identity_key_normalizes_only_impl_generics() {
        // impl: spec base form pairs with generic source form.
        assert_eq!(
            identity_key("impl", "RateLimiter"),
            identity_key("impl", "RateLimiter<C>"),
        );
        // non-impl: generics (if any) are part of identity, untouched.
        assert_eq!(identity_key("fn", "make<C>"), "make<C>");
    }

    #[test]
    fn normalize_impl_name_strips_trait_and_target_paths() {
        // F-013: path-qualified trait reduces to its last segment.
        assert_eq!(
            normalize_impl_name("fmt::Display for RateLimited"),
            "Display for RateLimited"
        );
        assert_eq!(
            normalize_impl_name("core::fmt::Display for RateLimited"),
            "Display for RateLimited"
        );
        assert_eq!(
            normalize_impl_name("std::error::Error for ConfigError"),
            "Error for ConfigError"
        );
        // Target path is normalized too.
        assert_eq!(
            normalize_impl_name("Display for crate::Foo"),
            "Display for Foo"
        );
        // Generics + paths compose (F-010 + F-013).
        assert_eq!(
            normalize_impl_name("core::convert::From<u32> for Foo"),
            "From for Foo"
        );
        // Bare names are unchanged.
        assert_eq!(
            normalize_impl_name("Display for RateLimited"),
            "Display for RateLimited"
        );
        assert_eq!(normalize_impl_name("RateLimiter"), "RateLimiter");
    }

    #[test]
    fn identity_key_pairs_bare_and_qualified_trait_impl() {
        // The dogfood case: spec `Display for X` ≡ source `fmt::Display for X`.
        assert_eq!(
            identity_key("impl", "Display for RateLimited"),
            identity_key("impl", "fmt::Display for RateLimited"),
        );
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
        let items = parse_spec_items(&v, "test", &rust_kinds()).unwrap();
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.kind, "re_export");
        assert_eq!(it.name, "Reexp");
        assert!(it.extras.contains_key("target"));
        assert!(it.extras.contains_key("visibility"));
        assert!(!it.extras.contains_key("kind"));
    }

    #[test]
    fn is_test_item_reads_test_flag_from_extras() {
        let toml_text = r#"
[[items]]
name = "drained_bucket_refuses_until_refill"
kind = "fn"
description = "unit test"
test = true
test_kind = "unit"
verifies = ["REQ-BUCKET-001"]

[[items]]
name = "TokenBucket::try_take"
kind = "fn"
description = "public method"
satisfies = ["REQ-BUCKET-001"]
"#;
        let v: toml::Value = toml::from_str(toml_text).unwrap();
        let items = parse_spec_items(&v, "test", &rust_kinds()).unwrap();
        let test_item = items
            .iter()
            .find(|i| i.extras.contains_key("test"))
            .unwrap();
        assert!(
            is_test_item(test_item),
            "test = true item must be a test item"
        );
        let method = items
            .iter()
            .find(|i| i.name == "TokenBucket::try_take")
            .unwrap();
        assert!(
            !is_test_item(method),
            "non-test item must not be a test item"
        );
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
        let err = parse_spec_items(&v, "test", &rust_kinds()).unwrap_err();
        assert!(format!("{err:#}").contains("ITEM-102"));
        assert!(format!("{err:#}").contains("lambda"));
    }
}
