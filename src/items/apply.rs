//! Three-stage sync — Stage 3: `items apply`.
//!
//! Reads an approved proposal file, validates the approval gate, and
//! mutates `specs/modules/<Name>.toml` per each entry's recommendation.
//! Uses `toml_edit` to round-trip the spec module — comments, key
//! ordering, and inline-table whitespace outside the `[[items]]` array
//! are preserved.
//!
//! After a successful apply, the proposal file is renamed to
//! `<module>.applied.yaml` with `status: applied` + `applied_at`
//! timestamp — that's the audit trail. Re-running `items propose`
//! after an apply lands a fresh `.proposal.yaml` (no `--clobber`
//! needed because the file doesn't exist anymore).
//!
//! Error codes:
//! - `ITEM-301` schema_version mismatch
//! - `ITEM-302` `approved: true` not set
//! - `ITEM-303` approver missing
//! - `ITEM-304` approved_at missing
//! - `ITEM-305` an entry's recommendation is still `pending`
//! - `ITEM-306` add-to-spec without `proposed_spec`
//! - `ITEM-307` proposed_spec is not a YAML mapping
//! - `ITEM-308` proposed_spec missing required `kind`/`name`
//! - `ITEM-309` target [[items]] entry not found for remove/rename
//! - `ITEM-310` proposal file read / parse failure
//! - `ITEM-311` module spec read / parse failure
//! - `ITEM-312` module spec write failure
//! - `ITEM-313` audit log rename failure
//! - `ITEM-314` unknown action
//! - `ITEM-315` rename without `rename_to`

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Serialize;

use crate::items::propose::ProposalDocument;
use crate::layout::LayoutContext;

#[derive(Debug, Serialize)]
pub struct ApplyReport {
    pub module: String,
    pub module_spec_path: PathBuf,
    pub audit_path: PathBuf,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub renamed: Vec<String>,
    pub skipped: Vec<String>,
}

impl ApplyReport {
    pub fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("module: {}\n", self.module));
        s.push_str(&format!("spec: {}\n", self.module_spec_path.display()));
        s.push_str(&format!("audit: {}\n", self.audit_path.display()));
        let line = |label: &str, items: &[String]| -> String {
            if items.is_empty() {
                format!("  {label}: 0\n")
            } else {
                format!("  {label}: {} ({})\n", items.len(), items.join(", "))
            }
        };
        s.push_str(&line("added", &self.added));
        s.push_str(&line("removed", &self.removed));
        s.push_str(&line("renamed", &self.renamed));
        s.push_str(&line("skipped", &self.skipped));
        s
    }
}

/// Apply a proposal to its module spec. Validates the full approval
/// gate before touching the spec; partial-failure scenarios past the
/// gate are protected by toml_edit doing all mutations in memory and
/// only writing on full success.
pub fn apply_proposal(
    layout: &LayoutContext,
    module_name: &str,
    proposal_path: &Path,
) -> Result<ApplyReport> {
    let proposal_text = std::fs::read_to_string(proposal_path)
        .with_context(|| format!("[ITEM-310] reading proposal at {}", proposal_path.display()))?;
    let mut doc: ProposalDocument = serde_yaml::from_str(&proposal_text)
        .with_context(|| format!("[ITEM-310] parsing proposal at {}", proposal_path.display()))?;

    if doc.schema_version != 1 {
        return Err(anyhow!(
            "[ITEM-301] proposal schema_version = {} (expected 1); regenerate with `items propose`",
            doc.schema_version
        ));
    }
    if doc.module != module_name {
        return Err(anyhow!(
            "[ITEM-310] proposal `module: {}` does not match --module {}",
            doc.module,
            module_name
        ));
    }
    if !doc.approved {
        return Err(anyhow!(
            "[ITEM-302] proposal at {} is not approved (`approved: false`); \
             review entries, set `approved: true` + `approver` + `approved_at`, \
             then re-run `items apply`",
            proposal_path.display()
        ));
    }
    let approver = doc.approver.as_deref().ok_or_else(|| {
        anyhow!(
            "[ITEM-303] proposal is approved but `approver` field is missing; \
             add `approver: <name-or-handle>` to the top of the file"
        )
    })?;
    let approved_at = doc.approved_at.as_deref().ok_or_else(|| {
        anyhow!(
            "[ITEM-304] proposal is approved but `approved_at` field is missing; \
             add `approved_at: <ISO-8601>` to the top of the file"
        )
    })?;

    // Validate every entry has a non-pending action and required fields.
    for entry in &doc.entries {
        match entry.recommendation.action.as_str() {
            "skip" | "remove-from-spec" => {}
            "rename" => {
                let to = entry
                    .recommendation
                    .rename_to
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or("");
                if to.is_empty() {
                    return Err(anyhow!(
                        "[ITEM-315] entry {} has action `rename` but no (non-empty) \
                         `rename_to`; add `rename_to: <new-name>` to the recommendation",
                        entry.id
                    ));
                }
            }
            "add-to-spec" => {
                let ps = entry.recommendation.proposed_spec.as_ref().ok_or_else(|| {
                    anyhow!(
                        "[ITEM-306] entry {} has action `{}` but no `proposed_spec`",
                        entry.id,
                        entry.recommendation.action
                    )
                })?;
                let map = ps.as_mapping().ok_or_else(|| {
                    anyhow!(
                        "[ITEM-307] entry {}: proposed_spec must be a YAML mapping",
                        entry.id
                    )
                })?;
                let has = |k: &str| {
                    map.get(serde_yaml::Value::String(k.into()))
                        .and_then(|v| v.as_str())
                        .is_some()
                };
                if !has("kind") || !has("name") {
                    return Err(anyhow!(
                        "[ITEM-308] entry {}: proposed_spec missing required `kind` or `name`",
                        entry.id
                    ));
                }
            }
            "pending" => {
                return Err(anyhow!(
                    "[ITEM-305] entry {} still has action `pending`; resolve it (skip / \
                     add-to-spec / remove-from-spec / rename) before applying",
                    entry.id
                ));
            }
            other => {
                return Err(anyhow!(
                    "[ITEM-314] entry {}: unknown action `{}` (expected one of \
                     skip / add-to-spec / remove-from-spec / rename)",
                    entry.id,
                    other
                ));
            }
        }
    }

    // Read module spec via toml_edit so comments/whitespace round-trip.
    let module_path = layout.modules_dir.join(format!("{module_name}.toml"));
    let module_text = std::fs::read_to_string(&module_path)
        .with_context(|| format!("[ITEM-311] reading module spec {}", module_path.display()))?;
    let mut spec_doc: toml_edit::DocumentMut = module_text
        .parse()
        .with_context(|| format!("[ITEM-311] parsing module spec {}", module_path.display()))?;

    let mut added: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    let mut renamed: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();

    for entry in &doc.entries {
        match entry.recommendation.action.as_str() {
            "skip" => skipped.push(entry.id.clone()),
            "remove-from-spec" => {
                if remove_item(&mut spec_doc, &entry.kind, &entry.name)? {
                    removed.push(entry.id.clone());
                } else {
                    return Err(anyhow!(
                        "[ITEM-309] entry {}: cannot remove (kind={}, name={}) — not present \
                         in {}",
                        entry.id,
                        entry.kind,
                        entry.name,
                        module_path.display()
                    ));
                }
            }
            "add-to-spec" => {
                let ps = entry.recommendation.proposed_spec.as_ref().unwrap();
                add_item(&mut spec_doc, ps)?;
                added.push(entry.id.clone());
            }
            "rename" => {
                // Validated non-empty above.
                let new_name = entry.recommendation.rename_to.as_deref().unwrap().trim();
                if rename_item(&mut spec_doc, &entry.kind, &entry.name, new_name)? {
                    renamed.push(entry.id.clone());
                } else {
                    return Err(anyhow!(
                        "[ITEM-309] entry {}: cannot rename (kind={}, name={}) — not present \
                         in {}",
                        entry.id,
                        entry.kind,
                        entry.name,
                        module_path.display()
                    ));
                }
            }
            _ => unreachable!("validated above"),
        }
    }

    std::fs::write(&module_path, spec_doc.to_string())
        .with_context(|| format!("[ITEM-312] writing module spec {}", module_path.display()))?;

    // Flip proposal status, add applied_at, rename to .applied.yaml.
    doc.status = "applied".into();
    let applied_at = chrono::Utc::now().to_rfc3339();
    // Same instant as the `applied_at` inside the file, so the name and the
    // contents cannot disagree about when this apply happened.
    let audit_path = audit_path_at(layout, module_name, &applied_at);
    let audit_yaml = serde_yaml::to_string(&AuditDocument {
        proposal: &doc,
        applied_at: &applied_at,
        approver,
        approved_at,
        summary: ApplySummary {
            added: added.len(),
            removed: removed.len(),
            renamed: renamed.len(),
            skipped: skipped.len(),
        },
    })
    .context("[ITEM-313] serializing audit log")?;
    if let Some(parent) = audit_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("[ITEM-313] creating audit dir {}", parent.display()))?;
    }
    std::fs::write(&audit_path, audit_yaml)
        .with_context(|| format!("[ITEM-313] writing audit log {}", audit_path.display()))?;
    // Remove the proposal file now that the audit log carries the
    // record. Future `items propose` lands a fresh .proposal.yaml.
    if proposal_path.exists() && proposal_path != audit_path {
        std::fs::remove_file(proposal_path).with_context(|| {
            format!(
                "[ITEM-313] removing applied proposal at {}",
                proposal_path.display()
            )
        })?;
    }

    Ok(ApplyReport {
        module: module_name.to_string(),
        module_spec_path: super::util::relative_to(&layout.project_root, &module_path),
        audit_path: super::util::relative_to(&layout.project_root, &audit_path),
        added,
        removed,
        renamed,
        skipped,
    })
}

#[derive(Debug, Serialize)]
struct AuditDocument<'a> {
    #[serde(flatten)]
    proposal: &'a ProposalDocument,
    applied_at: &'a str,
    approver: &'a str,
    approved_at: &'a str,
    summary: ApplySummary,
}

#[derive(Debug, Serialize)]
struct ApplySummary {
    added: usize,
    removed: usize,
    renamed: usize,
    skipped: usize,
}

/// `<specdev_root>/upstream/items-sync/<module>.<stamp>.applied.yaml` —
/// the audit path for one apply, stamped with the moment it happened.
///
/// A fixed `<module>.applied.yaml` meant the second apply silently discarded
/// the first one's provenance — which entries were applied, and when. The
/// sibling writer (`items::propose::write_proposal`) already refuses to clobber
/// without `--clobber`; an audit log that overwrites itself is worse than one
/// that refuses, because losing history is the one thing it exists to prevent.
///
/// The stamp is colon-free so the name is portable to filesystems that reject
/// `:` (Windows, some network mounts).
pub fn audit_path_at(layout: &LayoutContext, module_name: &str, stamp: &str) -> PathBuf {
    let safe = stamp.replace(':', "-");
    layout
        .specdev_root
        .join("upstream")
        .join("items-sync")
        .join(format!("{module_name}.{safe}.applied.yaml"))
}

fn items_array_mut(doc: &mut toml_edit::DocumentMut) -> Result<&mut toml_edit::ArrayOfTables> {
    if !doc.contains_key("items") {
        // Create an empty `[[items]]` array via insertion of a single
        // table, then take a mutable handle.
        let arr = toml_edit::ArrayOfTables::new();
        doc.insert("items", toml_edit::Item::ArrayOfTables(arr));
    }
    doc["items"].as_array_of_tables_mut().ok_or_else(|| {
        anyhow!("[ITEM-311] module spec `items` field is not an array of tables; cannot apply")
    })
}

fn remove_item(doc: &mut toml_edit::DocumentMut, kind: &str, name: &str) -> Result<bool> {
    let arr = items_array_mut(doc)?;
    let pos = arr.iter().position(|t| {
        t.get("kind").and_then(|v| v.as_str()) == Some(kind)
            && t.get("name").and_then(|v| v.as_str()) == Some(name)
    });
    if let Some(idx) = pos {
        arr.remove(idx);
        Ok(true)
    } else {
        Ok(false)
    }
}

fn add_item(doc: &mut toml_edit::DocumentMut, proposed: &serde_yaml::Value) -> Result<()> {
    let table = yaml_to_toml_table(proposed)?;
    let arr = items_array_mut(doc)?;
    arr.push(table);
    Ok(())
}

/// Rename an existing `[[items]]` entry in place: find it by its current
/// `(kind, name)` and rewrite *only* the `name` value to `new_name`. Every
/// other key — `satisfies`, `verifies`, `description`, kind-discriminated
/// extras — is left untouched, which is the whole point of a distinct
/// `rename` action: it preserves the requirement/verification trace that a
/// remove + add would force the author to re-author by hand. Returns `false`
/// if no item matches (caller maps that to `ITEM-309`).
fn rename_item(
    doc: &mut toml_edit::DocumentMut,
    kind: &str,
    name: &str,
    new_name: &str,
) -> Result<bool> {
    let arr = items_array_mut(doc)?;
    let pos = arr.iter().position(|t| {
        t.get("kind").and_then(|v| v.as_str()) == Some(kind)
            && t.get("name").and_then(|v| v.as_str()) == Some(name)
    });
    match pos {
        Some(idx) => {
            if let Some(t) = arr.get_mut(idx) {
                t["name"] = toml_edit::value(new_name);
            }
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Convert a YAML mapping (the proposal's `proposed_spec`) into a
/// `toml_edit::Table` with stable, author-friendly key order: kind →
/// name → description → satisfies → everything else
/// (alphabetical). Matches the convention codegen prose recommends.
fn yaml_to_toml_table(v: &serde_yaml::Value) -> Result<toml_edit::Table> {
    let map = v
        .as_mapping()
        .ok_or_else(|| anyhow!("[ITEM-307] proposed_spec must be a YAML mapping"))?;

    let mut table = toml_edit::Table::new();

    let pinned = ["kind", "name", "description", "satisfies"];
    for key in pinned {
        if let Some(val) = map.get(serde_yaml::Value::String(key.into())) {
            table.insert(key, yaml_to_toml_item(val)?);
        }
    }
    let mut leftover_keys: Vec<&str> = map
        .iter()
        .filter_map(|(k, _)| k.as_str())
        .filter(|k| !pinned.contains(k))
        .collect();
    leftover_keys.sort();
    for k in leftover_keys {
        let val = map.get(serde_yaml::Value::String(k.into())).unwrap();
        table.insert(k, yaml_to_toml_item(val)?);
    }
    Ok(table)
}

fn yaml_to_toml_item(v: &serde_yaml::Value) -> Result<toml_edit::Item> {
    let value = yaml_to_toml_value(v)?;
    Ok(toml_edit::Item::Value(value))
}

fn yaml_to_toml_value(v: &serde_yaml::Value) -> Result<toml_edit::Value> {
    use toml_edit::Value;
    Ok(match v {
        serde_yaml::Value::Null => Value::from(""),
        serde_yaml::Value::Bool(b) => Value::from(*b),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from(i)
            } else if let Some(f) = n.as_f64() {
                Value::from(f)
            } else {
                Value::from(n.to_string())
            }
        }
        serde_yaml::Value::String(s) => Value::from(s.as_str()),
        serde_yaml::Value::Sequence(seq) => {
            let mut arr = toml_edit::Array::new();
            for item in seq {
                arr.push(yaml_to_toml_value(item)?);
            }
            Value::Array(arr)
        }
        serde_yaml::Value::Mapping(m) => {
            let mut inline = toml_edit::InlineTable::new();
            for (k, v) in m {
                let key = k.as_str().ok_or_else(|| {
                    anyhow!("[ITEM-307] proposed_spec mapping key must be a string")
                })?;
                inline.insert(key, yaml_to_toml_value(v)?);
            }
            Value::InlineTable(inline)
        }
        serde_yaml::Value::Tagged(t) => yaml_to_toml_value(&t.value)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml_map(pairs: &[(&str, serde_yaml::Value)]) -> serde_yaml::Value {
        let mut m = serde_yaml::Mapping::new();
        for (k, v) in pairs {
            m.insert(serde_yaml::Value::String((*k).into()), v.clone());
        }
        serde_yaml::Value::Mapping(m)
    }

    #[test]
    fn yaml_to_toml_table_pins_canonical_field_order() {
        let v = yaml_map(&[
            ("zzz_extra", serde_yaml::Value::String("z".into())),
            ("name", serde_yaml::Value::String("foo".into())),
            ("kind", serde_yaml::Value::String("fn".into())),
            ("description", serde_yaml::Value::String("d".into())),
        ]);
        let t = yaml_to_toml_table(&v).unwrap();
        let keys: Vec<&str> = t.iter().map(|(k, _)| k).collect();
        assert_eq!(keys, vec!["kind", "name", "description", "zzz_extra"]);
    }

    #[test]
    fn rename_item_rewrites_name_in_place_preserving_other_keys() {
        let mut doc: toml_edit::DocumentMut = r#"name = "Foo"

[[items]]
kind = "fn"
name = "refill"
description = "Refill the bucket."
satisfies = ["DEMO-1"]
"#
        .parse()
        .unwrap();
        let ok = rename_item(&mut doc, "fn", "refill", "apply_refill").unwrap();
        assert!(ok);
        let out = doc.to_string();
        assert!(
            out.contains("name = \"apply_refill\""),
            "name not rewritten:\n{out}"
        );
        assert!(
            !out.contains("name = \"refill\""),
            "old name lingers:\n{out}"
        );
        // Trace and description ride through untouched.
        assert!(
            out.contains("satisfies = [\"DEMO-1\"]"),
            "satisfies lost:\n{out}"
        );
        assert!(
            out.contains("description = \"Refill the bucket.\""),
            "description lost:\n{out}"
        );
    }

    #[test]
    fn rename_item_returns_false_when_no_match() {
        let mut doc: toml_edit::DocumentMut = r#"name = "Foo"

[[items]]
kind = "fn"
name = "refill"
"#
        .parse()
        .unwrap();
        // Wrong kind → no match.
        assert!(!rename_item(&mut doc, "struct", "refill", "x").unwrap());
        // Wrong name → no match.
        assert!(!rename_item(&mut doc, "fn", "nope", "x").unwrap());
    }

    #[test]
    fn yaml_to_toml_value_handles_satisfies_array() {
        let v = serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("REQ-1".into()),
            serde_yaml::Value::String("REQ-2".into()),
        ]);
        let t = yaml_to_toml_value(&v).unwrap();
        let arr = t.as_array().expect("array");
        assert_eq!(arr.len(), 2);
    }
}
