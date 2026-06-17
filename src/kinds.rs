//! Kind-path parser.
//!
//! The path grammar defines three forms:
//!
//! ```text
//! unqualified      = Type | relation-name
//! block-qualified  = block "." (Type | relation-name)
//! bridge-qualified = block "+" block "." relation-name
//!
//! block            = [a-z][a-z0-9-]*     (lowercase, hyphens allowed, no '+')
//! Type             = [A-Z][A-Za-z0-9]*   (PascalCase for entity kinds)
//! relation-name    = [a-z][a-z0-9_]*     (snake_case for relation kinds)
//! ```
//!
//! Bridge-qualified paths require `left < right` lexicographically. This
//! parser enforces lex-order at parse time. The reduction loader
//! independently enforces lex-order on bridge filesystem names — see
//! `reductions::resolve`. Lex-order is enforced in these two places
//! intentionally: both checks point at the same human error class but
//! catch it at different lifecycle stages.
//!
//! Bridges do NOT declare entities — only relations — so the bridge form
//! with a PascalCase tail (`embassy+lvgl.Renders`) is rejected.

use std::fmt;

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KindPath {
    /// Unqualified `Type` (entity) or `relation_name` (relation).
    /// Resolves under the file's enclosing namespace at load time.
    Unqualified(String),

    /// Block-qualified path: `<block>.<Type>` or `<block>.<relation>`.
    Block { block: String, name: String },

    /// Bridge-qualified path: `<a>+<b>.<relation>` with `a < b`. Bridges
    /// declare relations only, so the tail is always snake_case.
    Bridge {
        left: String,
        right: String,
        relation: String,
    },
}

impl KindPath {
    /// True if this path names an entity (PascalCase tail). Bridges are
    /// always relations.
    pub fn is_entity(&self) -> bool {
        match self {
            KindPath::Unqualified(s) => is_pascal_case(s),
            KindPath::Block { name, .. } => is_pascal_case(name),
            KindPath::Bridge { .. } => false,
        }
    }

    /// True if this path names a relation (snake_case tail).
    pub fn is_relation(&self) -> bool {
        match self {
            KindPath::Unqualified(s) => is_snake_case(s),
            KindPath::Block { name, .. } => is_snake_case(name),
            KindPath::Bridge { .. } => true,
        }
    }
}

impl fmt::Display for KindPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KindPath::Unqualified(s) => write!(f, "{s}"),
            KindPath::Block { block, name } => write!(f, "{block}.{name}"),
            KindPath::Bridge {
                left,
                right,
                relation,
            } => write!(f, "{left}+{right}.{relation}"),
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum KindPathError {
    #[error("empty kind path")]
    Empty,
    #[error(
        "invalid kind path `{0}`: must match `Type`, `<block>.<Kind>`, or `<a>+<b>.<relation>`"
    )]
    InvalidGrammar(String),
    #[error("invalid block name `{0}`: must match `[a-z][a-z0-9-]*`")]
    InvalidBlockName(String),
    #[error("invalid entity name `{0}`: must match `[A-Z][A-Za-z0-9]*` (PascalCase)")]
    InvalidEntityName(String),
    #[error("invalid relation name `{0}`: must match `[a-z][a-z0-9_]*` (snake_case)")]
    InvalidRelationName(String),
    #[error(
        "bridge owner names must be lex-ordered: `{left}+{right}.{relation}` should be \
         `{right}+{left}.{relation}` (left < right)"
    )]
    BridgeNotLexOrdered {
        left: String,
        right: String,
        relation: String,
    },
    #[error(
        "bridge path `{0}` declares an entity tail (PascalCase); bridges declare relations only"
    )]
    BridgeEntityNotAllowed(String),
    #[error("bridge owner names must differ: `{0}+{0}.<relation>` is not a valid bridge")]
    BridgeSelfPair(String),
}

/// Parse a kind-path string into one of the three forms.
pub fn parse(s: &str) -> Result<KindPath, KindPathError> {
    if s.is_empty() {
        return Err(KindPathError::Empty);
    }

    match s.split_once('.') {
        None => {
            // Unqualified: must be PascalCase entity OR snake_case relation.
            if is_pascal_case(s) || is_snake_case(s) {
                Ok(KindPath::Unqualified(s.to_string()))
            } else {
                Err(KindPathError::InvalidGrammar(s.to_string()))
            }
        }
        Some((owner, tail)) => {
            if tail.is_empty() {
                return Err(KindPathError::InvalidGrammar(s.to_string()));
            }
            if let Some((left, right)) = owner.split_once('+') {
                // Bridge-qualified.
                if !is_block_name(left) {
                    return Err(KindPathError::InvalidBlockName(left.to_string()));
                }
                if !is_block_name(right) {
                    return Err(KindPathError::InvalidBlockName(right.to_string()));
                }
                if left == right {
                    return Err(KindPathError::BridgeSelfPair(left.to_string()));
                }
                if is_pascal_case(tail) {
                    return Err(KindPathError::BridgeEntityNotAllowed(s.to_string()));
                }
                if !is_snake_case(tail) {
                    return Err(KindPathError::InvalidRelationName(tail.to_string()));
                }
                if left >= right {
                    return Err(KindPathError::BridgeNotLexOrdered {
                        left: left.to_string(),
                        right: right.to_string(),
                        relation: tail.to_string(),
                    });
                }
                Ok(KindPath::Bridge {
                    left: left.to_string(),
                    right: right.to_string(),
                    relation: tail.to_string(),
                })
            } else {
                // Block-qualified.
                if !is_block_name(owner) {
                    return Err(KindPathError::InvalidBlockName(owner.to_string()));
                }
                if !(is_pascal_case(tail) || is_snake_case(tail)) {
                    return Err(KindPathError::InvalidGrammar(s.to_string()));
                }
                Ok(KindPath::Block {
                    block: owner.to_string(),
                    name: tail.to_string(),
                })
            }
        }
    }
}

/// Parse a same-namespace shorthand. Accepts only the unqualified form
/// (PascalCase or snake_case bare name). Used by the resolver when
/// rewriting `kind: Task` → `<block>.Task` based on file location.
pub fn parse_unqualified(s: &str) -> Result<String, KindPathError> {
    if s.is_empty() {
        return Err(KindPathError::Empty);
    }
    if s.contains('.') || s.contains('+') {
        return Err(KindPathError::InvalidGrammar(s.to_string()));
    }
    if is_pascal_case(s) || is_snake_case(s) {
        Ok(s.to_string())
    } else if s.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
        Err(KindPathError::InvalidEntityName(s.to_string()))
    } else {
        Err(KindPathError::InvalidRelationName(s.to_string()))
    }
}

fn is_block_name(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_pascal_case(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric())
}

fn is_snake_case(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unqualified_pascal_case_entity() {
        assert_eq!(
            parse("Module").unwrap(),
            KindPath::Unqualified("Module".into())
        );
        assert_eq!(parse("Task").unwrap(), KindPath::Unqualified("Task".into()));
        assert_eq!(
            parse("StateMachine").unwrap(),
            KindPath::Unqualified("StateMachine".into())
        );
    }

    #[test]
    fn unqualified_snake_case_relation() {
        assert_eq!(
            parse("depends_on").unwrap(),
            KindPath::Unqualified("depends_on".into())
        );
        assert_eq!(
            parse("contains").unwrap(),
            KindPath::Unqualified("contains".into())
        );
    }

    #[test]
    fn block_qualified_entity() {
        let parsed = parse("embassy.Task").unwrap();
        assert_eq!(
            parsed,
            KindPath::Block {
                block: "embassy".into(),
                name: "Task".into(),
            }
        );
        assert!(parsed.is_entity());
        assert!(!parsed.is_relation());
    }

    #[test]
    fn block_qualified_relation() {
        let parsed = parse("embassy.spawns").unwrap();
        assert_eq!(
            parsed,
            KindPath::Block {
                block: "embassy".into(),
                name: "spawns".into(),
            }
        );
        assert!(!parsed.is_entity());
        assert!(parsed.is_relation());
    }

    #[test]
    fn bridge_qualified_relation() {
        let parsed = parse("embassy+lvgl.renders").unwrap();
        assert_eq!(
            parsed,
            KindPath::Bridge {
                left: "embassy".into(),
                right: "lvgl".into(),
                relation: "renders".into(),
            }
        );
    }

    #[test]
    fn bridge_lex_order_violation() {
        let err = parse("lvgl+embassy.renders").unwrap_err();
        assert!(matches!(err, KindPathError::BridgeNotLexOrdered { .. }));
    }

    #[test]
    fn bridge_entity_tail_rejected() {
        let err = parse("embassy+lvgl.Renders").unwrap_err();
        assert!(matches!(err, KindPathError::BridgeEntityNotAllowed(_)));
    }

    #[test]
    fn bridge_self_pair_rejected() {
        let err = parse("embassy+embassy.spawns").unwrap_err();
        assert!(matches!(err, KindPathError::BridgeSelfPair(_)));
    }

    #[test]
    fn block_name_with_hyphen_allowed() {
        let parsed = parse("stm32f4-rs.Peripheral").unwrap();
        assert_eq!(
            parsed,
            KindPath::Block {
                block: "stm32f4-rs".into(),
                name: "Peripheral".into(),
            }
        );
    }

    #[test]
    fn invalid_block_name_uppercase() {
        let err = parse("Embassy.Task").unwrap_err();
        // Uppercase 'E' fails block-name regex; we report block invalid.
        assert!(matches!(err, KindPathError::InvalidBlockName(_)));
    }

    #[test]
    fn empty_string_rejected() {
        assert!(matches!(parse(""), Err(KindPathError::Empty)));
    }

    #[test]
    fn trailing_dot_rejected() {
        assert!(matches!(
            parse("embassy."),
            Err(KindPathError::InvalidGrammar(_))
        ));
    }

    #[test]
    fn display_round_trip() {
        for s in [
            "Module",
            "depends_on",
            "embassy.Task",
            "embassy+lvgl.renders",
        ] {
            assert_eq!(parse(s).unwrap().to_string(), s);
        }
    }

    #[test]
    fn parse_unqualified_accepts_bare_names() {
        assert_eq!(parse_unqualified("Task").unwrap(), "Task");
        assert_eq!(parse_unqualified("depends_on").unwrap(), "depends_on");
    }

    #[test]
    fn parse_unqualified_rejects_qualified() {
        assert!(parse_unqualified("embassy.Task").is_err());
        assert!(parse_unqualified("a+b.rel").is_err());
    }
}
