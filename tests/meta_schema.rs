//! Task 1 of Phase 14 — the metaschema and its two closed-enum siblings
//! load via `SchemaRegistry` and the metaschema rejects a malformed
//! `reduction.yaml`.

use std::path::{Path, PathBuf};

use serde_json::json;
use specdev_analyzer::schema::SchemaRegistry;

fn meta_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("specdev/schema/meta/v1")
}

#[test]
fn meta_schemas_load_and_compile() {
    let reg = SchemaRegistry::load_dir(&meta_dir()).expect("load meta/v1");

    // Compiling the metaschema proves it is a well-formed JSON Schema 2020-12
    // (the jsonschema crate validates against the meta-schema during build).
    // `reduction.schema.yaml` $refs the other two, so a successful compile
    // also proves those are loadable and resolvable.
    let _ = reg
        .validate("reduction", &json!({}), "probe")
        .expect("metaschema compiles");
}

#[test]
fn metaschema_accepts_well_formed_reduction() {
    let reg = SchemaRegistry::load_dir(&meta_dir()).expect("load meta/v1");

    let good = json!({
        "metaschema_version": "v1",
        "entities": [
            {
                "kind": "module",
                "extends": "structural",
                "from": "specs/modules/*.toml",
                "name": "${.name}",
                "attrs": { "layer": "layer" }
            }
        ],
        "relations": [
            {
                "kind": "depends_on",
                "from_kind": "module",
                "to_kind": "module",
                "from_spec": "specs/modules/*.toml",
                "for_each": "depends_on[]",
                "endpoints": { "from": "${parent.name}", "to": "${.}" }
            }
        ]
    });

    let findings = reg
        .validate("reduction", &good, "good.yaml")
        .expect("validate");
    assert!(
        findings.is_empty(),
        "well-formed reduction rejected: {findings:?}"
    );
}

#[test]
fn metaschema_rejects_relation_missing_endpoints() {
    let reg = SchemaRegistry::load_dir(&meta_dir()).expect("load meta/v1");

    let bad = json!({
        "metaschema_version": "v1",
        "entities": [],
        "relations": [
            {
                "kind": "depends_on",
                "from_kind": "module",
                "to_kind": "module",
                "from_spec": "specs/modules/*.toml"
                // endpoints missing
            }
        ]
    });

    let findings = reg
        .validate("reduction", &bad, "bad.yaml")
        .expect("validate");
    assert!(
        !findings.is_empty(),
        "missing endpoints should have failed metaschema"
    );
}

#[test]
fn metaschema_rejects_unknown_root_kind() {
    let reg = SchemaRegistry::load_dir(&meta_dir()).expect("load meta/v1");

    let bad = json!({
        "metaschema_version": "v1",
        "entities": [
            {
                "kind": "module",
                "extends": "not_a_root_kind",
                "from": "specs/modules/*.toml",
                "name": "${.name}"
            }
        ],
        "relations": []
    });

    let findings = reg
        .validate("reduction", &bad, "bad.yaml")
        .expect("validate");
    assert!(!findings.is_empty(), "bogus extends should have failed");
}

#[test]
fn metaschema_rejects_unknown_relation_kind() {
    let reg = SchemaRegistry::load_dir(&meta_dir()).expect("load meta/v1");

    let bad = json!({
        "metaschema_version": "v1",
        "entities": [],
        "relations": [
            {
                "kind": "owns",   // considered-and-deferred per 10-metaschema.md
                "from_kind": "module",
                "to_kind": "module",
                "from_spec": "specs/modules/*.toml",
                "endpoints": { "from": "a", "to": "b" }
            }
        ]
    });

    let findings = reg
        .validate("reduction", &bad, "bad.yaml")
        .expect("validate");
    assert!(
        !findings.is_empty(),
        "relation `owns` is not in v1 vocabulary"
    );
}
