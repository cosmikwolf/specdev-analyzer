//! Canonical kind sets, loaded from the embedded metaschemas at startup.
//!
//! The canonical root-kind and relation-kind enums are closed at v1 (doc 10
//! §"Canonical root kinds" / §"Canonical relation vocabulary"). They form
//! the *base* vocabulary; blocks and bridges contribute path-qualified kinds
//! that classify under the base via `extends`.
//!
//! These sets are embedded in the analyzer (doc 15 §11 invariant 5: no
//! plugin lookup at analyze time). The embedded YAMLs at
//! `src/embedded/{root_kinds,relation_kinds}.schema.yaml` are the source of
//! truth; we parse them once at first use.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::Value as JsonValue;

const ROOT_KINDS_YAML: &str = include_str!("embedded/root_kinds.schema.yaml");
const RELATION_KINDS_YAML: &str = include_str!("embedded/relation_kinds.schema.yaml");

static CANONICAL_ROOT_KINDS_CELL: OnceLock<BTreeSet<String>> = OnceLock::new();
static CANONICAL_RELATION_KINDS_CELL: OnceLock<BTreeSet<String>> = OnceLock::new();

pub fn canonical_root_kinds() -> &'static BTreeSet<String> {
    CANONICAL_ROOT_KINDS_CELL.get_or_init(|| extract_enum(ROOT_KINDS_YAML, "RootKind"))
}

pub fn canonical_relation_kinds() -> &'static BTreeSet<String> {
    CANONICAL_RELATION_KINDS_CELL.get_or_init(|| extract_enum(RELATION_KINDS_YAML, "RelationKind"))
}

pub fn is_canonical_root(s: &str) -> bool {
    canonical_root_kinds().contains(s)
}

pub fn is_canonical_relation(s: &str) -> bool {
    canonical_relation_kinds().contains(s)
}

fn extract_enum(yaml: &str, def_name: &str) -> BTreeSet<String> {
    let value: JsonValue = serde_yaml::from_str(yaml)
        .unwrap_or_else(|e| panic!("embedded {def_name} YAML failed to parse: {e}"));
    let arr = value
        .pointer(&format!("/$defs/{def_name}/enum"))
        .and_then(JsonValue::as_array)
        .unwrap_or_else(|| panic!("embedded {def_name} YAML missing /$defs/{def_name}/enum"));
    arr.iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_kinds_are_the_seven_v1_values() {
        let expected: BTreeSet<String> = [
            "structural",
            "behavioral",
            "event",
            "state",
            "data",
            "statement",
            "grouping",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(canonical_root_kinds(), &expected);
    }

    #[test]
    fn relation_kinds_are_the_eight_v1_values() {
        let expected: BTreeSet<String> = [
            "contains",
            "depends_on",
            "connects",
            "satisfies",
            "transitions",
            "emits",
            "handles",
            "typed_as",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(canonical_relation_kinds(), &expected);
    }

    #[test]
    fn helpers_match_set_membership() {
        assert!(is_canonical_root("structural"));
        assert!(!is_canonical_root("Module"));
        assert!(!is_canonical_root("embassy.Task"));
        assert!(is_canonical_relation("depends_on"));
        assert!(!is_canonical_relation("spawns"));
        assert!(!is_canonical_relation("embassy+lvgl.renders"));
    }
}
