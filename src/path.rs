//! Path selector for JSON-like values.
//!
//! A selector is a slash-free string like
//!   `state_machines[].transitions[].from`
//! which descends object keys and, at each `[]`, iterates array elements.
//!
//! [`select`] returns a vector of [`Pick`]s — each holds the matched value
//! and a JSON pointer reflecting the concrete path it came from.

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
}

fn parse(selector: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for segment in selector.split('.') {
        if segment.is_empty() {
            continue;
        }
        // A segment may be `name`, `name[]`, or `name[][]` (for nested arrays).
        let mut rest = segment;
        let array_markers = segment.matches("[]").count();
        if let Some(head) = segment.split("[]").next() {
            if !head.is_empty() {
                steps.push(Step::Key(head.to_string()));
            }
            rest = &rest[head.len()..];
        }
        for _ in 0..array_markers {
            steps.push(Step::Each);
            // skip `[]`
            rest = &rest[2..];
        }
        let _ = rest; // remainder is empty by construction
    }
    steps
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
            Step::Key(name) => match value {
                Value::Object(map) => {
                    if let Some(inner) = map.get(name) {
                        let new_ptr = format!("{pointer}/{}", escape_ptr(name));
                        descend(inner, &steps[1..], new_ptr, out);
                    }
                }
                _ => {} // path misses — silent. Caller can distinguish empty result.
            },
            Step::Each => match value {
                Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        let new_ptr = format!("{pointer}/{i}");
                        descend(item, &steps[1..], new_ptr, out);
                    }
                }
                _ => {}
            },
        },
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
}
