//! File:line:col-shaped error payloads.
//!
//! The `<path>:<line>:<col>: <message>` error format is the CLI baseline.
//! Errors carry a `LocatedError` payload
//! when the originating subsystem (TOML parser, YAML parser, JSON Schema
//! validator) can pinpoint a source position; the top-level `report_error`
//! helper renders that payload as the first line so a user can jump to the
//! offending location with no further plumbing.

use std::fmt;
use std::path::{Path, PathBuf};

use jsonschema::error::ValidationErrorKind;
use jsonschema::ValidationError;

/// Closed-enum threshold for full-options listing per F-012.
///
/// jsonschema 0.46's default `Display` truncates enum mismatch messages to
/// `"a", "b" or N other candidates` once the option count exceeds 3. For
/// closed enums of `<= 8` values that's frustrating: the user has to open
/// the schema or a block README to see what's allowed. This threshold
/// keeps the output bounded (8 short string variants render comfortably
/// on one line) while covering every closed enum the shipped profiles
/// currently author.
const ENUM_LIST_ALL_THRESHOLD: usize = 8;

/// Format a `jsonschema::ValidationError` for inclusion in a SCHEMA finding.
///
/// Behavior matches the upstream `Display` impl except for closed-enum
/// mismatches: when the schema's `enum` has `<= ENUM_LIST_ALL_THRESHOLD`
/// values, every allowed value appears in the message instead of the
/// truncated `or N other candidates` suffix.
///
/// F-012 (2026-05-08 zfm dogfood): the egui panel-kind enum has 5 values
/// (`central`, `side_left`, `side_right`, `top`, `bottom`); the truncated
/// form (`"central", "side_left" or 3 other candidates`) hid the value
/// the user actually wanted. Listing all 5 saves a round-trip.
pub fn format_schema_error(err: &ValidationError<'_>) -> String {
    if let ValidationErrorKind::OneOfNotValid { context } = err.kind() {
        return format_one_of_failure(err, context);
    }
    // A `not` subschema that carries a `description` states why the value is
    // refused (a removed field, for example); print that instead of the
    // subschema's JSON.
    if let ValidationErrorKind::Not { schema } = err.kind() {
        if let Some(why) = schema.get("description").and_then(|d| d.as_str()) {
            let path = err.instance_path().to_string();
            let key = path.rsplit('/').next().unwrap_or_default();
            return format!("`{key}` is not allowed: {}", why.trim());
        }
    }
    if let ValidationErrorKind::Enum { options } = err.kind() {
        if let Some(arr) = options.as_array() {
            if arr.len() <= ENUM_LIST_ALL_THRESHOLD && !arr.is_empty() {
                let listed = format_enum_options(arr);
                return format!("{} is not one of {listed}", err.instance());
            }
        }
    }
    format!("{err}")
}

/// Maximum branches summarised after the closest one. Beyond this the tail is
/// counted rather than listed, so a wide `oneOf` cannot produce a message
/// nobody reads.
const ONE_OF_SUMMARY_LIMIT: usize = 3;

/// Explain a `oneOf` failure by naming the closest branch and what it wanted.
///
/// Upstream renders `{...whole instance...} is not valid under any of the
/// schemas listed in the 'oneOf' keyword` and stops. That names neither the
/// branch the author was aiming at nor the key they left out, so the advice
/// that led them there ("add items[]") terminates in an error they cannot act
/// on. The sub-errors are already in hand — `OneOfNotValid` carries one error
/// vector per branch — so the closest branch is simply the one with the
/// fewest unmet conditions.
///
/// Branches are reported by 1-based index, not by name: the error context
/// carries each branch's failures but not its `title`, and inventing a name
/// from the failures would be a guess. The unmet conditions identify the
/// branch in practice ("needs `satisfies`" is the declaration branch).
fn format_one_of_failure(
    err: &ValidationError<'_>,
    context: &[Vec<ValidationError<'static>>],
) -> String {
    if context.is_empty() {
        return format!("{err}");
    }

    // Rank by unmet-condition count. `min_by_key` keeps the first on a tie,
    // which is the earliest-declared branch — a stable, explainable choice.
    let mut ranked: Vec<(usize, &Vec<ValidationError<'static>>)> =
        context.iter().enumerate().collect();
    ranked.sort_by_key(|(idx, errs)| (errs.len(), *idx));

    let (closest_idx, closest_errs) = ranked[0];
    let closest = format!(
        "closest is branch {} of {}, which needs: {}",
        closest_idx + 1,
        context.len(),
        summarize_branch(closest_errs)
    );

    let others: Vec<String> = ranked
        .iter()
        .skip(1)
        .take(ONE_OF_SUMMARY_LIMIT)
        .map(|(idx, errs)| format!("branch {} needs {}", idx + 1, summarize_branch(errs)))
        .collect();

    let mut msg = format!(
        "{} is not valid under any of the {} schemas in 'oneOf' — {}",
        err.instance(),
        context.len(),
        closest
    );
    if !others.is_empty() {
        msg.push_str(&format!(". Also tried: {}", others.join("; ")));
    }
    let remaining = ranked.len().saturating_sub(1 + ONE_OF_SUMMARY_LIMIT);
    if remaining > 0 {
        msg.push_str(&format!(" (and {remaining} more)"));
    }
    msg
}

/// One branch's unmet conditions, shortest-useful form.
///
/// A missing `required` key is by far the common case and is rendered as the
/// bare key name, because that is the edit the author has to make. Anything
/// else falls back to the upstream sentence.
fn summarize_branch(errs: &[ValidationError<'static>]) -> String {
    if errs.is_empty() {
        return "nothing (it matched, but another branch matched too)".to_string();
    }
    let parts: Vec<String> = errs
        .iter()
        .map(|e| match e.kind() {
            ValidationErrorKind::Required { property } => {
                let key = property.as_str().unwrap_or("<unknown>");
                format!("`{key}`")
            }
            _ => format!("{e}"),
        })
        .collect();
    parts.join(", ")
}

fn format_enum_options(options: &[serde_json::Value]) -> String {
    let total = options.len();
    let mut buf = String::new();
    for (i, opt) in options.iter().enumerate() {
        if i == 0 {
            buf.push_str(&format!("{opt}"));
        } else if i == total - 1 {
            buf.push_str(&format!(" or {opt}"));
        } else {
            buf.push_str(&format!(", {opt}"));
        }
    }
    buf
}

#[derive(Debug, Clone, thiserror::Error)]
pub struct LocatedError {
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl fmt::Display for LocatedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line == 0 && self.col == 0 {
            write!(f, "{}: {}", self.path.display(), self.message)
        } else if self.col == 0 {
            write!(f, "{}:{}: {}", self.path.display(), self.line, self.message)
        } else {
            write!(
                f,
                "{}:{}:{}: {}",
                self.path.display(),
                self.line,
                self.col,
                self.message
            )
        }
    }
}

impl LocatedError {
    pub fn new(path: impl Into<PathBuf>, line: u32, col: u32, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line,
            col,
            message: message.into(),
        }
    }

    /// Build a `LocatedError` from the byte span of a TOML parse error. Walks
    /// the source text to convert the byte offset into a (line, col) pair.
    pub fn from_toml_error(path: &Path, source: &str, err: &toml::de::Error) -> Self {
        let (line, col) = match err.span() {
            Some(span) => byte_offset_to_line_col(source, span.start),
            None => (1, 1),
        };
        Self {
            path: path.to_path_buf(),
            line,
            col,
            message: err.message().to_string(),
        }
    }

    /// Build a `LocatedError` from a `serde_yaml` parse error. The crate
    /// exposes `Location` with line / column directly when present.
    pub fn from_yaml_error(path: &Path, err: &serde_yaml::Error) -> Self {
        let (line, col) = match err.location() {
            Some(loc) => (loc.line() as u32, loc.column() as u32),
            None => (1, 1),
        };
        Self {
            path: path.to_path_buf(),
            line,
            col,
            message: err.to_string(),
        }
    }
}

fn byte_offset_to_line_col(source: &str, offset: usize) -> (u32, u32) {
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    let bytes = source.as_bytes();
    let end = offset.min(bytes.len());
    for &b in &bytes[..end] {
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Render an `anyhow::Error` chain to stderr. When the chain carries a
/// [`LocatedError`], it appears as the top line so the user gets a
/// machine-parseable `<path>:<line>:<col>: <message>` first.
pub fn report_error(err: &anyhow::Error) {
    if let Some(loc) = err.downcast_ref::<LocatedError>() {
        eprintln!("{loc}");
        // Walk the rest of the chain and emit context, skipping the located
        // payload itself.
        for cause in err.chain().skip(1) {
            if cause.downcast_ref::<LocatedError>().is_some() {
                continue;
            }
            eprintln!("  caused by: {cause}");
        }
    } else {
        eprintln!("{err:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn first_error_for(schema: &serde_json::Value, instance: &serde_json::Value) -> String {
        let validator = jsonschema::options().build(schema).unwrap();
        let err = validator.iter_errors(instance).next().unwrap();
        format_schema_error(&err)
    }

    /// The shape that motivated this: the items schema's three-branch
    /// `oneOf`, where an ordinary declaration omits `satisfies`. Upstream
    /// names neither the branch nor the key.
    fn items_like_one_of() -> serde_json::Value {
        json!({
            "oneOf": [
                {"title": "re_export", "required": ["target"],
                 "properties": {"kind": {"const": "re_export"}}},
                {"title": "test-function", "required": ["test", "verifies"]},
                {"title": "declaration", "required": ["satisfies"]}
            ]
        })
    }

    #[test]
    fn one_of_failure_names_the_closest_branch_and_its_missing_key() {
        let instance = json!({
            "name": "connectDevice",
            "kind": "function",
            "description": "Connects to a device."
        });
        let msg = first_error_for(&items_like_one_of(), &instance);
        assert!(
            msg.contains("closest is branch 3 of 3"),
            "must name the branch it ranked closest: {msg}"
        );
        assert!(
            msg.contains("`satisfies`"),
            "must name the key the author has to add — this is the whole point: {msg}"
        );
        assert!(
            !msg.contains("is not valid under any of the schemas listed in the 'oneOf' keyword"),
            "must not fall through to the upstream sentence: {msg}"
        );
    }

    #[test]
    fn one_of_failure_also_summarises_the_branches_it_rejected() {
        let instance = json!({"name": "x", "kind": "function"});
        let msg = first_error_for(&items_like_one_of(), &instance);
        assert!(
            msg.contains("Also tried"),
            "rejected branches must appear: {msg}"
        );
        assert!(msg.contains("`target`"), "re_export branch's key: {msg}");
        assert!(msg.contains("`verifies`"), "test branch's key: {msg}");
    }

    #[test]
    fn one_of_ranks_by_unmet_condition_count_not_declaration_order() {
        // Branch 1 is declared first but wants two keys; branch 2 wants one.
        // Ranking by count must pick branch 2, or "closest" means nothing.
        let schema = json!({
            "oneOf": [
                {"required": ["alpha", "beta"]},
                {"required": ["gamma"]}
            ]
        });
        let msg = first_error_for(&schema, &json!({}));
        assert!(
            msg.contains("closest is branch 2 of 2"),
            "fewest unmet conditions must win: {msg}"
        );
        assert!(msg.contains("`gamma`"), "{msg}");
    }

    #[test]
    fn one_of_with_many_branches_bounds_the_summary() {
        // Six branches: closest + 3 summarised + "(and 2 more)". A message
        // nobody reads is the same as no message.
        let schema = json!({
            "oneOf": [
                {"required": ["a"]}, {"required": ["b"]}, {"required": ["c"]},
                {"required": ["d"]}, {"required": ["e"]}, {"required": ["f"]}
            ]
        });
        let msg = first_error_for(&schema, &json!({}));
        assert!(
            msg.contains("and 2 more"),
            "tail must be counted, not listed: {msg}"
        );
    }

    #[test]
    fn a_non_required_branch_failure_keeps_the_upstream_sentence() {
        // Not every branch fails on a missing key. When it fails on a const
        // mismatch there is no key to name, so the upstream wording is the
        // honest fallback rather than an invented one.
        let schema = json!({
            "oneOf": [
                {"properties": {"kind": {"const": "alpha"}}, "required": ["kind"]},
                {"properties": {"kind": {"const": "beta"}}, "required": ["kind"]}
            ]
        });
        let msg = first_error_for(&schema, &json!({"kind": "gamma"}));
        assert!(msg.contains("closest is branch 1 of 2"), "{msg}");
        assert!(
            msg.contains("alpha"),
            "the const it wanted must survive into the message: {msg}"
        );
    }

    #[test]
    fn enum_with_5_values_lists_all_in_message() {
        // F-012: closed enum of 5 values must be listed in full, not truncated.
        let schema = json!({
            "enum": ["central", "side_left", "side_right", "top", "bottom"]
        });
        let instance = json!("left");
        let msg = first_error_for(&schema, &instance);
        assert!(msg.contains("\"central\""), "missing first option: {msg}");
        assert!(msg.contains("\"side_left\""), "missing option: {msg}");
        assert!(msg.contains("\"side_right\""), "missing option: {msg}");
        assert!(msg.contains("\"top\""), "missing option: {msg}");
        assert!(msg.contains("\"bottom\""), "missing tail option: {msg}");
        assert!(
            !msg.contains("other candidates"),
            "must not truncate at 5 values: {msg}"
        );
    }

    #[test]
    fn enum_with_8_values_lists_all() {
        let schema = json!({
            "enum": ["a", "b", "c", "d", "e", "f", "g", "h"]
        });
        let instance = json!("z");
        let msg = first_error_for(&schema, &instance);
        for letter in [
            "\"a\"", "\"b\"", "\"c\"", "\"d\"", "\"e\"", "\"f\"", "\"g\"", "\"h\"",
        ] {
            assert!(msg.contains(letter), "missing {letter} in: {msg}");
        }
        assert!(
            !msg.contains("other candidates"),
            "must not truncate at 8: {msg}"
        );
    }

    #[test]
    fn enum_above_threshold_falls_back_to_truncation() {
        let schema = json!({
            "enum": ["a", "b", "c", "d", "e", "f", "g", "h", "i"]
        });
        let instance = json!("z");
        let msg = first_error_for(&schema, &instance);
        // Above threshold: defer to upstream Display, which truncates.
        assert!(
            msg.contains("other candidates"),
            "9 values must use truncated form: {msg}"
        );
    }

    #[test]
    fn enum_with_3_values_shipped_form_is_unchanged() {
        // Below threshold: upstream already lists all values; behavior is
        // identical via either path.
        let schema = json!({"enum": ["a", "b", "c"]});
        let instance = json!("z");
        let msg = first_error_for(&schema, &instance);
        assert!(msg.contains("\"a\""), "{msg}");
        assert!(msg.contains("\"b\""), "{msg}");
        assert!(msg.contains("\"c\""), "{msg}");
        assert!(!msg.contains("other candidates"), "{msg}");
    }

    #[test]
    fn non_enum_errors_pass_through_to_upstream_display() {
        // A `type` mismatch must format identically to upstream Display.
        let schema = json!({"type": "integer"});
        let instance = json!("not an int");
        let msg = first_error_for(&schema, &instance);
        let validator = jsonschema::options().build(&schema).unwrap();
        let upstream = format!("{}", validator.iter_errors(&instance).next().unwrap());
        assert_eq!(msg, upstream);
    }

    #[test]
    fn byte_offset_to_line_col_handles_multiline() {
        let src = "abc\nde\nfghi";
        assert_eq!(byte_offset_to_line_col(src, 0), (1, 1));
        assert_eq!(byte_offset_to_line_col(src, 4), (2, 1));
        assert_eq!(byte_offset_to_line_col(src, 7), (3, 1));
        assert_eq!(byte_offset_to_line_col(src, 9), (3, 3));
    }

    #[test]
    fn located_error_renders_with_line_col() {
        let e = LocatedError::new("/tmp/foo.toml", 7, 14, "expected string");
        assert_eq!(e.to_string(), "/tmp/foo.toml:7:14: expected string");
    }

    #[test]
    fn located_error_renders_without_col_when_zero() {
        let e = LocatedError::new("/tmp/foo.toml", 7, 0, "expected string");
        assert_eq!(e.to_string(), "/tmp/foo.toml:7: expected string");
    }
}
