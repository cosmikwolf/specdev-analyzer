//! `requirement_size`: a requirement states one behaviour.
//!
//! ISO/IEC/IEEE 29148 asks that a requirement be *singular* and the INCOSE
//! Guide to Writing Requirements (R18) one sentence with one thought; neither
//! sets a number. The number comes from an earlier PRD checker, which caps
//! an entry at 120 words, so the two tools agree; words are counted as that
//! checker counts them: whitespace-separated tokens that hold a letter or a
//! digit. A longer text is a warning, not an
//! error: length is a sign of several behaviours, not a proof. Retired
//! requirements are exempt.

use std::collections::BTreeMap;

use crate::domain::Severity;
use crate::entries::{is_retired_file, EntryKind, EntryStore};
use crate::layout::LayoutContext;
use crate::report::{Finding, Report};

pub const RULE: &str = "requirement_size";

/// The most words a requirement's `text` may have before the warning.
pub const MAX_WORDS: usize = 120;

/// Words: whitespace-separated tokens that hold a letter or a digit.
pub fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

/// One warning per live requirement over [`MAX_WORDS`].
pub fn check(layout: &LayoutContext, rules: &BTreeMap<String, Severity>, report: &mut Report) {
    let sev = rules.get(RULE).copied().unwrap_or(Severity::Warning);
    for e in EntryStore::for_layout(layout).load_lenient(EntryKind::Requirement) {
        if is_retired_file(&e.file) {
            continue;
        }
        let (Some(id), Some(text)) = (e.id(), e.value.get("text").and_then(toml::Value::as_str))
        else {
            continue;
        };
        let n = word_count(text);
        if n > MAX_WORDS {
            report.push(
                Finding::new(
                    RULE,
                    sev,
                    format!(
                        "requirement `{id}` has {n} words; at most {MAX_WORDS}: a requirement states one \
                         behaviour (ISO 29148 singular, INCOSE R18); split it, and give each part its own \
                         `verified_by`"
                    ),
                )
                .with_file(e.file.display().to_string())
                .with_entity(id),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::word_count;

    #[test]
    fn counts_tokens_that_hold_a_letter_or_digit() {
        assert_eq!(word_count("one two  three"), 3);
        assert_eq!(word_count("a — b - c"), 3);
        assert_eq!(word_count("REQ-1, 750 ms."), 3);
        assert_eq!(word_count("  "), 0);
    }
}
