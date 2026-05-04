//! Path selector and template evaluator for JSON-like values.
//!
//! # Selector grammar (v1)
//!
//! ```text
//! selector  := root? segment ("." segment)*
//! root      := "$"                    // optional; ignorable — paths are
//!                                     // already rooted at the value passed
//!                                     // to `select`.
//! segment   := key modifier*
//!            | "." modifier*          // bare modifiers on the leading `.`
//!                                     // (used in templates to mean "current").
//! key       := [A-Za-z_][A-Za-z0-9_]*
//! modifier  := "[]"                   // iterate every array element
//!            | "[" N "]"              // array element at index N
//! ```
//!
//! Example: `state_machines[].transitions[0].from` picks the `from` field of
//! the first transition of every state machine.
//!
//! # Template grammar (v1)
//!
//! Templates are literal strings that may embed `${<path>}` interpolations,
//! where `<path>` follows the selector grammar above. Two extra path roots
//! work inside templates:
//!
//! * `.` — the current iterator element (set by a `for_each` in the
//!   reduction spec).
//! * `parent.<k>` — the parent-entity scope. Currently only `parent.name` is
//!   defined. One level of parent only — `parent.parent.*` is rejected in v1.
//!
//! Templates have no conditionals, filters, or defaults. Add them via a
//! metaschema version bump if a real need shows up (see
//! `specdev-next/10-metaschema.md:163-171`).

use std::fmt::Write as _;

use anyhow::{bail, Result};
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct Pick {
    pub pointer: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Key(String),
    Each,
    Index(usize),
}

fn parse(selector: &str) -> Vec<Step> {
    // Strip leading `$` or `$.` — selectors are already rooted at the value
    // passed to `select`. This keeps pre-v1 callers (`state_machines[].from`)
    // working while accepting the metaschema's JSONPath-ish form (`$.state_machines`).
    let selector = selector.strip_prefix('$').unwrap_or(selector);
    let selector = selector.strip_prefix('.').unwrap_or(selector);

    // Split on `.` but respect double-quoted segments (used for keys with
    // non-identifier characters, e.g. bridge names like
    // `bindings."egui+nih-plug".parameter_widgets[]`). Quoted-string keys
    // are stripped of their surrounding quotes before being used as JSON
    // object keys; the quotes never reach the JSON-key match.
    let mut segments: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in selector.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            '.' if !in_quotes => {
                if !current.is_empty() {
                    segments.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }

    let mut steps = Vec::new();
    for segment in segments {
        if segment.is_empty() {
            continue;
        }
        parse_segment(&segment, &mut steps);
    }
    steps
}

fn parse_segment(segment: &str, out: &mut Vec<Step>) {
    let bytes = segment.as_bytes();

    // Leading identifier (or quoted-key — strip surrounding `"` before
    // emitting the Key step so the JSON-key match sees the raw string).
    let mut head_end = 0;
    while head_end < bytes.len() && bytes[head_end] != b'[' {
        head_end += 1;
    }
    if head_end > 0 {
        let head = &segment[..head_end];
        let key = if head.len() >= 2 && head.starts_with('"') && head.ends_with('"') {
            head[1..head.len() - 1].to_string()
        } else {
            head.to_string()
        };
        out.push(Step::Key(key));
    }
    let mut i = head_end;

    // Modifiers: `[]` or `[N]`, possibly repeated.
    while i < bytes.len() && bytes[i] == b'[' {
        let close = segment[i..].find(']').map(|n| i + n);
        match close {
            Some(j) => {
                let inside = &segment[i + 1..j];
                if inside.is_empty() {
                    out.push(Step::Each);
                } else if let Ok(n) = inside.parse::<usize>() {
                    out.push(Step::Index(n));
                } else {
                    // Unknown bracket body — treat the whole segment as a literal key
                    // rather than silently dropping it. Caller sees an empty pick.
                    out.clear();
                    out.push(Step::Key(segment.to_string()));
                    return;
                }
                i = j + 1;
            }
            None => {
                // Unmatched `[` — literal fallback, as above.
                out.clear();
                out.push(Step::Key(segment.to_string()));
                return;
            }
        }
    }
}

pub fn select(root: &Value, selector: &str) -> Vec<Pick> {
    let steps = parse(selector);
    let mut out = Vec::new();
    descend(root, &steps, String::new(), &mut out);
    out
}

fn descend(value: &Value, steps: &[Step], pointer: String, out: &mut Vec<Pick>) {
    match steps.first() {
        None => out.push(Pick {
            pointer,
            value: value.clone(),
        }),
        Some(step) => match step {
            Step::Key(name) => {
                if let Value::Object(map) = value {
                    if let Some(inner) = map.get(name) {
                        let new_ptr = format!("{pointer}/{}", escape_ptr(name));
                        descend(inner, &steps[1..], new_ptr, out);
                    }
                }
                // else: path misses silently. Caller can distinguish empty result.
            }
            Step::Each => {
                if let Value::Array(items) = value {
                    for (i, item) in items.iter().enumerate() {
                        let new_ptr = format!("{pointer}/{i}");
                        descend(item, &steps[1..], new_ptr, out);
                    }
                }
            }
            Step::Index(n) => {
                if let Value::Array(items) = value {
                    if let Some(item) = items.get(*n) {
                        let new_ptr = format!("{pointer}/{n}");
                        descend(item, &steps[1..], new_ptr, out);
                    }
                }
            }
        },
    }
}

/// Scope for template evaluation. A reduction.yaml invocation constructs this
/// per materialized entity/relation.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    /// The file-root document (whatever `select` sees by default).
    pub root: &'a Value,
    /// The current `for_each` element, if any. `${.field}` reads from here.
    pub current: Option<&'a Value>,
    /// Materialized name of the parent entity, if any. `${parent.name}` reads
    /// from here. Only `parent.name` is defined in v1.
    pub parent_name: Option<&'a str>,
}

/// Evaluate a template string. `${<path>}` interpolations are replaced by the
/// resolved value rendered as a string; literal text is copied through.
pub fn eval_template(template: &str, scope: &Scope<'_>) -> Result<String> {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            let close = template[i + 2..]
                .find('}')
                .map(|n| i + 2 + n)
                .ok_or_else(|| anyhow::anyhow!("unclosed `${{` in template: {template:?}"))?;
            let expr = &template[i + 2..close];
            let resolved = resolve_template_path(expr, scope)?;
            out.push_str(&resolved);
            i = close + 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    Ok(out)
}

fn resolve_template_path(expr: &str, scope: &Scope<'_>) -> Result<String> {
    // `parent.<k>` — only `parent.name` defined in v1.
    if let Some(rest) = expr.strip_prefix("parent.") {
        return match rest {
            "name" => scope
                .parent_name
                .map(String::from)
                .ok_or_else(|| anyhow::anyhow!("`parent.name` referenced but no parent in scope")),
            s if s == "parent" || s.starts_with("parent.") => {
                bail!("only one level of `parent` is defined in v1 (offending path: `${{{expr}}}`)")
            }
            other => {
                bail!("unsupported parent field `{other}` — only `parent.name` is defined in v1")
            }
        };
    }

    // `.` alone — current element as-is.
    if expr == "." {
        let current = scope.current.ok_or_else(|| {
            anyhow::anyhow!("`${{.}}` referenced but no current element in scope")
        })?;
        return Ok(render_scalar(current));
    }

    // `.foo.bar` — current element's field.
    if let Some(rest) = expr.strip_prefix('.') {
        let current = scope.current.ok_or_else(|| {
            anyhow::anyhow!("`${{.}}` referenced but no current element in scope")
        })?;
        let picks = select(current, rest);
        return pick_to_string(&picks, expr);
    }

    // Otherwise: path rooted at the file root.
    let picks = select(scope.root, expr);
    pick_to_string(&picks, expr)
}

fn pick_to_string(picks: &[Pick], expr: &str) -> Result<String> {
    match picks.len() {
        0 => bail!("template path `${{{expr}}}` resolved to nothing"),
        1 => Ok(render_scalar(&picks[0].value)),
        n => bail!("template path `${{{expr}}}` resolved to {n} values; expected 1"),
    }
}

fn render_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => {
            let mut out = String::new();
            let _ = write!(out, "{other}");
            out
        }
    }
}

/// Ancestor pointer up to (but not including) the n-th array-index component.
///
/// Used for grouping picks by their parent array element — e.g. "all picks
/// that came from the same state_machines[i]".
pub fn ancestor_at_array(pointer: &str, nth: usize) -> Option<String> {
    let mut count = 0;
    for (idx, ch) in pointer.char_indices() {
        if ch == '/' {
            let tail = &pointer[idx + 1..];
            let next_slash = tail.find('/').map(|i| idx + 1 + i).unwrap_or(pointer.len());
            let segment = &pointer[idx + 1..next_slash];
            if segment.chars().all(|c| c.is_ascii_digit()) && !segment.is_empty() {
                if count == nth {
                    return Some(pointer[..next_slash].to_string());
                }
                count += 1;
            }
        }
    }
    None
}

fn escape_ptr(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_scalar() {
        let v = json!({ "name": "abc" });
        let picks = select(&v, "name");
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].pointer, "/name");
        assert_eq!(picks[0].value, json!("abc"));
    }

    #[test]
    fn picks_array_elements() {
        let v = json!({ "xs": [{ "n": "a" }, { "n": "b" }] });
        let picks = select(&v, "xs[].n");
        assert_eq!(picks.len(), 2);
        assert_eq!(picks[0].pointer, "/xs/0/n");
        assert_eq!(picks[1].value, json!("b"));
    }

    #[test]
    fn picks_nested_arrays() {
        let v = json!({
            "sms": [
                { "name": "A", "ts": [{ "from": "s0" }, { "from": "s1" }] },
                { "name": "B", "ts": [{ "from": "q0" }] },
            ],
        });
        let picks = select(&v, "sms[].ts[].from");
        assert_eq!(picks.len(), 3);
        assert_eq!(picks[2].pointer, "/sms/1/ts/0/from");
        assert_eq!(picks[2].value, json!("q0"));
    }

    #[test]
    fn ancestor_prefix_first_array() {
        let p = "/sms/1/ts/0/from";
        assert_eq!(ancestor_at_array(p, 0).as_deref(), Some("/sms/1"));
    }

    #[test]
    fn picks_with_dollar_root() {
        let v = json!({ "name": "abc" });
        let picks = select(&v, "$.name");
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].pointer, "/name");
        assert_eq!(picks[0].value, json!("abc"));
    }

    #[test]
    fn picks_array_index() {
        let v = json!({ "xs": [{ "n": "a" }, { "n": "b" }, { "n": "c" }] });
        let picks = select(&v, "xs[1].n");
        assert_eq!(picks.len(), 1);
        assert_eq!(picks[0].pointer, "/xs/1/n");
        assert_eq!(picks[0].value, json!("b"));
    }

    #[test]
    fn picks_array_index_out_of_range() {
        let v = json!({ "xs": [{ "n": "a" }] });
        let picks = select(&v, "xs[5].n");
        assert!(picks.is_empty());
    }

    #[test]
    fn template_current_field() {
        let current = json!({ "name": "s0" });
        let root = json!({});
        let scope = Scope {
            root: &root,
            current: Some(&current),
            parent_name: None,
        };
        let out = eval_template("state_${.name}", &scope).expect("eval");
        assert_eq!(out, "state_s0");
    }

    #[test]
    fn template_current_dot_alone() {
        let current = json!("raw_value");
        let root = json!({});
        let scope = Scope {
            root: &root,
            current: Some(&current),
            parent_name: None,
        };
        let out = eval_template("${.}", &scope).expect("eval");
        assert_eq!(out, "raw_value");
    }

    #[test]
    fn template_parent_name() {
        let root = json!({});
        let scope = Scope {
            root: &root,
            current: None,
            parent_name: Some("ModA"),
        };
        let out = eval_template("${parent.name}", &scope).expect("eval");
        assert_eq!(out, "ModA");
    }

    #[test]
    fn template_parent_name_and_current() {
        let current = json!({ "name": "leaf" });
        let root = json!({});
        let scope = Scope {
            root: &root,
            current: Some(&current),
            parent_name: Some("ModA"),
        };
        let out = eval_template("${parent.name}.${.name}", &scope).expect("eval");
        assert_eq!(out, "ModA.leaf");
    }

    #[test]
    fn template_rejects_deep_parent() {
        let root = json!({});
        let scope = Scope {
            root: &root,
            current: None,
            parent_name: Some("Mod"),
        };
        let err = eval_template("${parent.parent.name}", &scope).unwrap_err();
        assert!(
            err.to_string().contains("one level"),
            "expected one-level rejection, got: {err}"
        );
    }

    #[test]
    fn template_unresolved_path_errors() {
        let root = json!({ "name": "root" });
        let scope = Scope {
            root: &root,
            current: None,
            parent_name: None,
        };
        let err = eval_template("${does.not.exist}", &scope).unwrap_err();
        assert!(
            err.to_string().contains("resolved to nothing"),
            "expected not-found error, got: {err}"
        );
    }

    #[test]
    fn template_unclosed_interp_errors() {
        let root = json!({});
        let scope = Scope {
            root: &root,
            current: None,
            parent_name: None,
        };
        let err = eval_template("prefix_${.name", &scope).unwrap_err();
        assert!(
            err.to_string().contains("unclosed"),
            "expected unclosed error, got: {err}"
        );
    }
}
