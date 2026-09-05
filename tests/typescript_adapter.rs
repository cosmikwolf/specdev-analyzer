//! TypeScript language admission: adapter resolution, the two-grammar
//! split (`.ts` → typescript, `.tsx` → tsx), embedded audit query compile
//! + captures under BOTH grammars over real source, parameter parsing,
//! lockstep between the two `language` schema enums, and the no-items-
//! vocabulary-yet refusal.
//!
//! The syntax-coverage fixtures under `tests/fixtures/typescript/` must
//! parse with zero ERROR nodes under their own grammar. The `.ts` fixture
//! deliberately carries a `<T>x` assertion, which the TSX grammar cannot
//! parse — that cross-grammar failure is asserted too, because it is the
//! reason the language spans two grammars at all.

use std::collections::BTreeSet;
use std::path::Path;

use specdev_analyzer::items::extract::extract_items_from_source;
use specdev_analyzer::lang::{adapter_for, LanguageAdapter};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

const TS_FIXTURE: &str = include_str!("fixtures/typescript/syntax_coverage.ts");
const TSX_FIXTURE: &str = include_str!("fixtures/typescript/syntax_coverage.tsx");

fn ts() -> Box<dyn LanguageAdapter> {
    adapter_for("typescript").expect("typescript adapter resolves")
}

fn parse(adapter: &dyn LanguageAdapter, ext: &str, src: &str) -> tree_sitter::Tree {
    let mut parser = Parser::new();
    parser
        .set_language(&adapter.tree_sitter_language_for(ext))
        .unwrap();
    parser.parse(src, None).unwrap()
}

fn captures(adapter: &dyn LanguageAdapter, ext: &str, src: &str) -> BTreeSet<(String, String)> {
    let language = adapter.tree_sitter_language_for(ext);
    let tree = parse(adapter, ext, src);
    let query = Query::new(&language, adapter.embedded_query())
        .unwrap_or_else(|e| panic!("audit.scm must compile under the `{ext}` grammar: {e}"));
    let mut out = BTreeSet::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), src.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let name = query.capture_names()[cap.index as usize].to_string();
            let text = src[cap.node.byte_range()].to_string();
            out.insert((name, text));
        }
    }
    out
}

#[test]
fn adapter_resolves_with_typescript_surface() {
    let a = ts();
    assert_eq!(a.name(), "typescript");
    assert_eq!(a.file_extension(), "ts");
    assert_eq!(a.file_extensions(), vec!["ts", "tsx"]);
    assert_eq!(a.path_separator(), ".");
    assert_eq!(
        a.module_file_candidates("event_card"),
        vec![
            "event_card.ts",
            "event_card.tsx",
            "event_card/index.ts",
            "event_card/index.tsx"
        ]
    );
    assert_eq!(
        a.lockfile_candidates(),
        &["pnpm-lock.yaml", "package-lock.json", "yarn.lock"]
    );
    assert!(a.toolchain_file_candidates().is_empty());
    // Single-grammar languages keep one extension and one grammar.
    assert_eq!(adapter_for("rust").unwrap().file_extensions(), vec!["rs"]);
    assert_eq!(adapter_for("dart").unwrap().file_extensions(), vec!["dart"]);
}

#[test]
fn both_fixtures_parse_clean_under_their_own_grammar() {
    let a = ts();
    let ts_tree = parse(a.as_ref(), "ts", TS_FIXTURE);
    assert!(
        !ts_tree.root_node().has_error(),
        "syntax_coverage.ts must parse with zero ERROR nodes under the typescript grammar"
    );
    let tsx_tree = parse(a.as_ref(), "tsx", TSX_FIXTURE);
    assert!(
        !tsx_tree.root_node().has_error(),
        "syntax_coverage.tsx must parse with zero ERROR nodes under the tsx grammar"
    );
}

#[test]
fn the_two_grammars_are_not_interchangeable() {
    // The reason one language spans two grammars: `<T>x` assertions fail
    // under TSX and JSX fails under TypeScript. Measured, not recalled.
    let a = ts();
    let ts_under_tsx = parse(a.as_ref(), "tsx", TS_FIXTURE);
    assert!(
        ts_under_tsx.root_node().has_error(),
        "the .ts fixture carries a `<T>x` assertion the tsx grammar cannot parse"
    );
    let tsx_under_ts = parse(a.as_ref(), "ts", TSX_FIXTURE);
    assert!(
        tsx_under_ts.root_node().has_error(),
        "the .tsx fixture carries JSX the typescript grammar cannot parse"
    );
    // Unknown extensions fall back to the primary (typescript) grammar.
    let fallback = parse(a.as_ref(), "mts", TS_FIXTURE);
    assert!(!fallback.root_node().has_error());
}

#[test]
fn embedded_audit_query_compiles_and_captures_under_both_grammars() {
    let a = ts();
    let c = captures(a.as_ref(), "ts", TS_FIXTURE);
    let has = |cap: &str, text: &str| c.contains(&(cap.to_string(), text.to_string()));

    assert!(has("fn.name", "clamp01"), "{c:?}");
    assert!(has("fn.name", "load"), "{c:?}");
    assert!(
        has("fn.name", "helper"),
        "non-exported fns are audit surface too: {c:?}"
    );
    // Overload signatures are `function_signature` nodes; only the
    // implementation is a `function_declaration`. One capture, not three.
    assert_eq!(
        c.iter()
            .filter(|(n, t)| n == "fn.name" && t == "over")
            .count(),
        1,
        "{c:?}"
    );
    assert!(has("class.name", "Circle"), "{c:?}");
    assert!(has("class.name", "Store"), "default-exported class: {c:?}");
    assert!(has("abstract_class.name", "Shape"), "{c:?}");
    assert!(has("interface.name", "Reading"), "{c:?}");
    assert!(has("interface.name", "Hidden"), "{c:?}");
    assert!(has("type_alias.name", "Result"), "{c:?}");
    assert!(has("enum.name", "Color"), "{c:?}");
    assert!(has("binding.name", "LIMIT"), "exported const: {c:?}");
    assert!(has("binding.name", "mutable"), "module-private let: {c:?}");
    assert!(
        !has("binding.name", "first"),
        "destructured bindings are skipped: {c:?}"
    );
    // Methods are `method_definition`, never top-level fns.
    assert!(!has("fn.name", "area"), "{c:?}");
    assert!(!has("fn.name", "unit"), "{c:?}");

    // Same query text, other grammar.
    let cx = captures(a.as_ref(), "tsx", TSX_FIXTURE);
    let hasx = |cap: &str, text: &str| cx.contains(&(cap.to_string(), text.to_string()));
    assert!(hasx("fn.name", "EventCard"), "{cx:?}");
    assert!(hasx("fn.name", "Page"), "{cx:?}");
    assert!(hasx("class.name", "Counter"), "{cx:?}");
    assert!(hasx("interface.name", "CardProps"), "{cx:?}");
    assert!(hasx("type_alias.name", "Variant"), "{cx:?}");
    assert!(hasx("enum.name", "Size"), "{cx:?}");
    assert!(hasx("binding.name", "Badge"), "{cx:?}");
}

#[test]
fn parse_parameter_reads_name_and_type() {
    let a = ts();
    let tree = parse(a.as_ref(), "ts", TS_FIXTURE);
    let mut params = Vec::new();
    collect_params(tree.root_node(), TS_FIXTURE, a.as_ref(), &mut params);
    let find = |n: &str| {
        params
            .iter()
            .find(|p| p.name == n)
            .unwrap_or_else(|| panic!("parameter `{n}` not parsed; got {params:?}"))
            .clone()
    };
    assert_eq!(find("x").type_name, "number");
    assert_eq!(find("lo").type_name, "number");
    assert_eq!(find("path").type_name, "string");
    assert_eq!(find("d").type_name, "Dirent");
    // The rest parameter's `pattern` is a `rest_pattern`, not an identifier.
    assert!(params.iter().all(|p| p.name != "rest"), "{params:?}");
}

#[test]
fn public_name_rule_rejects_hash_private_segments() {
    let a = ts();
    assert!(a.item_is_public_name("Circle.area"));
    assert!(a.item_is_public_name("clamp01"));
    assert!(!a.item_is_public_name("Circle.#cache"));
    assert!(!a.item_is_public_name("#hard"));
}

#[test]
fn extract_items_refuses_until_the_items_vocabulary_ships() {
    // No `items.scm` in this tier yet: the extractor must say so by name,
    // not panic or return an empty list that reads as "no items".
    let a = ts();
    let err = extract_items_from_source(TS_FIXTURE, "ts", a.as_ref(), None).unwrap_err();
    let s = format!("{err:#}");
    assert!(
        s.contains("no items.scm query for language `typescript`"),
        "{s}"
    );
}

/// The two `language` enums (project spec + profile manifest) drifted once;
/// this locks them in lockstep and asserts both admit `typescript`.
#[test]
fn spec_and_profile_language_enums_agree_and_admit_typescript() {
    let embedded = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/embedded");
    let spec: serde_json::Value =
        serde_yaml::from_str(&std::fs::read_to_string(embedded.join("spec.schema.yaml")).unwrap())
            .unwrap();
    let profile: serde_json::Value = serde_yaml::from_str(
        &std::fs::read_to_string(embedded.join("profile.schema.yaml")).unwrap(),
    )
    .unwrap();
    let spec_enum = spec
        .pointer("/properties/project/properties/language/enum")
        .expect("spec.schema.yaml language enum");
    let profile_enum = profile
        .pointer("/properties/language/enum")
        .expect("profile.schema.yaml language enum");
    assert_eq!(
        spec_enum, profile_enum,
        "language enums must stay in lockstep"
    );
    let admits = spec_enum
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some("typescript"));
    assert!(admits, "{spec_enum:?}");
    // And every enum member resolves to an adapter — the enum is the
    // registry's public face.
    for v in spec_enum.as_array().unwrap() {
        let name = v.as_str().unwrap();
        assert!(
            adapter_for(name).is_some(),
            "enum admits `{name}` but no adapter resolves it"
        );
    }
}

fn collect_params(
    node: tree_sitter::Node,
    src: &str,
    adapter: &dyn LanguageAdapter,
    out: &mut Vec<specdev_analyzer::lang::ParsedParameter>,
) {
    if node.kind() == adapter.parameter_node_kind() || node.kind() == "optional_parameter" {
        if let Some(p) = adapter.parse_parameter(node, src) {
            out.push(p);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_params(child, src, adapter, out);
    }
}
