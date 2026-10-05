//! Plugin-author lint: each block's and bridge's rule pack (`checks.toml`).
//!
//! - **`PACK-LINT-001`** — the pack does not parse, or fails
//!   `checks.schema.yaml`.
//! - **`PACK-LINT-002`** — a code check's query file is missing, its
//!   language has no adapter, or it does not compile for that language.
//! - **`PACK-LINT-003`** — a query lacks a capture its code check names.
//! - **`PACK-LINT-004`** — a graph check names an unknown primitive.
//! - **`PACK-LINT-005`** — a rule name lacks the unit-name prefix
//!   (`<unit>.<rule>`). Packs share one severity namespace with the
//!   project's `domain.toml`, so an unprefixed name can collide with a
//!   profile's rule, and a project override would hit the wrong check. The
//!   generic code-check rules are exempt.
//!
//! A unit with no `checks.toml` ships no pack; nothing is checked.

use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::Query;

use crate::code_checks::{ATTRIBUTE_MATCHES, ENTITY_IN_CODE, KEY_AMBIGUOUS};
use crate::inline::CHECKS_FILE;
use crate::validate::GRAPH_CHECK_PRIMITIVES;

/// Lint every block's and bridge's pack under `plugin_root`. Returns sorted,
/// pre-coded offender lines (`<unit>: [CODE] ...`).
pub fn lint_packs(plugin_root: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for kind in ["blocks", "bridges"] {
        let base = plugin_root.join(kind);
        let Ok(rd) = std::fs::read_dir(&base) else { continue };
        let mut units: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.is_dir()).collect();
        units.sort();
        for unit in units {
            lint_unit(&unit, &mut out)?;
        }
    }
    out.sort();
    Ok(out)
}

fn lint_unit(unit: &Path, out: &mut Vec<String>) -> Result<()> {
    let path = unit.join(CHECKS_FILE);
    if !path.is_file() {
        return Ok(());
    }
    let name = unit.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let pack = match crate::packs::parse(&text) {
        Ok(p) => p,
        Err(e) => {
            out.push(format!("{name}: [PACK-LINT-001] {CHECKS_FILE} {e:#}"));
            return Ok(());
        }
    };
    let prefix = format!("{name}.");
    let prefixed = |rule: &str| rule.starts_with(&prefix) && rule.len() > prefix.len();
    for check in &pack.graph_checks {
        let rule = check.rule.as_deref().unwrap_or("?");
        if !GRAPH_CHECK_PRIMITIVES.contains(&check.kind.as_str()) {
            out.push(format!(
                "{name}: [PACK-LINT-004] graph check `{rule}` names unknown primitive `{}` — known: {}",
                check.kind,
                GRAPH_CHECK_PRIMITIVES.join(", ")
            ));
        }
        if !prefixed(rule) {
            out.push(format!("{name}: [PACK-LINT-005] graph check rule `{rule}` must be named `{prefix}<rule>`"));
        }
    }
    for rule in pack.rules.keys() {
        if !prefixed(rule) && ![ENTITY_IN_CODE, ATTRIBUTE_MATCHES, KEY_AMBIGUOUS].contains(&rule.as_str()) {
            out.push(format!("{name}: [PACK-LINT-005] [rules] entry `{rule}` must be named `{prefix}<rule>`"));
        }
    }
    for check in &pack.code_checks {
        let qpath = unit.join(&check.query);
        let Ok(src) = std::fs::read_to_string(&qpath) else {
            out.push(format!("{name}: [PACK-LINT-002] query {} is missing", check.query));
            continue;
        };
        let Some(adapter) = crate::lang::adapter_for(&check.language) else {
            out.push(format!("{name}: [PACK-LINT-002] query {}: no adapter for language `{}`", check.query, check.language));
            continue;
        };
        let mut wanted = vec![check.key_capture.as_str()];
        wanted.extend(check.arg_key_capture.as_deref());
        wanted.extend(check.arg_value_capture.as_deref());
        // Every grammar the language spans must accept the query.
        let mut exts = adapter.file_extensions();
        exts.dedup();
        for ext in exts {
            let lang = adapter.tree_sitter_language_for(ext);
            match Query::new(&lang, &src) {
                Err(e) => {
                    out.push(format!(
                        "{name}: [PACK-LINT-002] query {} does not compile for `{}` (.{ext}): {e}",
                        check.query, check.language
                    ));
                    break;
                }
                Ok(q) => {
                    let missing: Vec<String> = wanted
                        .iter()
                        .filter(|c| q.capture_index_for_name(c).is_none())
                        .map(|c| format!("@{c}"))
                        .collect();
                    if !missing.is_empty() {
                        out.push(format!(
                            "{name}: [PACK-LINT-003] query {} lacks the capture(s) its code check names: {}",
                            check.query,
                            missing.join(", ")
                        ));
                        break;
                    }
                }
            }
        }
    }
    Ok(())
}
