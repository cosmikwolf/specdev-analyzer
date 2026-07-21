//! Dart language admission: adapter resolution, embedded audit query
//! compile + captures over real Dart source, parameter parsing, and
//! lockstep between the two `language` schema enums.

use std::collections::BTreeSet;
use std::path::Path;

use specdev_analyzer::lang::{adapter_for, LanguageAdapter};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

const FIXTURE: &str = r#"
class RateLimiter {
  final int capacity;
  RateLimiter(this.capacity);
  bool tryConsume(int tokens, {String? label}) => true;
  int get remaining => 0;
  set remaining(int v) {}
}

mixin Poker {
  void poke() {}
}

enum Color { red, green }

extension ColorX on Color {
  String get pretty => name;
}

extension type Meters(double value) {
  double get feet => value * 3.28;
}

typedef Callback = void Function(int);

double clamp01(double x, [double lo = 0]) => x;
"#;

fn dart() -> Box<dyn LanguageAdapter> {
    adapter_for("dart").expect("dart adapter resolves")
}

#[test]
fn adapter_resolves_with_dart_surface() {
    let a = dart();
    assert_eq!(a.name(), "dart");
    assert_eq!(a.file_extension(), "dart");
    assert_eq!(a.path_separator(), ".");
    // Rust keeps the default separator.
    assert_eq!(adapter_for("rust").unwrap().path_separator(), "::");
}

#[test]
fn embedded_audit_query_compiles_and_captures_top_level_constructs() {
    let a = dart();
    let language = a.tree_sitter_language();
    let mut parser = Parser::new();
    parser.set_language(&language).unwrap();
    let tree = parser.parse(FIXTURE, None).unwrap();
    assert!(!tree.root_node().has_error(), "fixture must parse clean");

    let query = Query::new(&language, a.embedded_query())
        .expect("embedded audit.scm compiles against the dart grammar");

    let mut captured: BTreeSet<(String, String)> = BTreeSet::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), FIXTURE.as_bytes());
    while let Some(m) = matches.next() {
        for cap in m.captures {
            let name = query.capture_names()[cap.index as usize].to_string();
            let text = FIXTURE[cap.node.byte_range()].to_string();
            captured.insert((name, text));
        }
    }

    let has = |cap: &str, text: &str| {
        captured.contains(&(cap.to_string(), text.to_string()))
    };
    assert!(has("class.name", "RateLimiter"), "{captured:?}");
    assert!(has("mixin.name", "Poker"), "{captured:?}");
    assert!(has("enum.name", "Color"), "{captured:?}");
    assert!(has("extension.name", "ColorX"), "{captured:?}");
    assert!(has("extension_type.name", "Meters"), "{captured:?}");
    assert!(has("typedef.name", "Callback"), "{captured:?}");
    assert!(has("fn.name", "clamp01"), "{captured:?}");
    // Methods inside a class body are not top-level fns for the audit query.
    assert!(!has("fn.name", "tryConsume"), "{captured:?}");
}

#[test]
fn parse_parameter_reads_name_and_type() {
    let a = dart();
    let mut parser = Parser::new();
    parser.set_language(&a.tree_sitter_language()).unwrap();
    let tree = parser.parse(FIXTURE, None).unwrap();

    let mut params = Vec::new();
    collect_params(tree.root_node(), FIXTURE, a.as_ref(), &mut params);
    let find = |n: &str| {
        params
            .iter()
            .find(|p| p.name == n)
            .unwrap_or_else(|| panic!("parameter `{n}` not parsed; got {params:?}"))
            .clone()
    };
    assert_eq!(find("tokens").type_name, "int");
    assert_eq!(find("label").type_name, "String?");
    assert_eq!(find("x").type_name, "double");
}

fn collect_params(
    node: tree_sitter::Node,
    src: &str,
    adapter: &dyn LanguageAdapter,
    out: &mut Vec<specdev_analyzer::lang::ParsedParameter>,
) {
    if node.kind() == adapter.parameter_node_kind() {
        if let Some(p) = adapter.parse_parameter(node, src) {
            out.push(p);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_params(child, src, adapter, out);
    }
}

/// The two `language` enums (project spec + profile manifest) drifted once;
/// this locks them in lockstep and asserts both admit `dart`.
#[test]
fn spec_and_profile_language_enums_agree_and_admit_dart() {
    let embedded = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/embedded");
    let spec: serde_json::Value = serde_yaml::from_str(
        &std::fs::read_to_string(embedded.join("spec.schema.yaml")).unwrap(),
    )
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
        "spec.toml and profile.yaml language enums must stay in lockstep"
    );
    let langs: Vec<&str> = spec_enum
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(langs.contains(&"dart"), "{langs:?}");
    // Every enum entry must resolve to an adapter — an enum value without
    // an adapter is dead admission surface.
    for lang in &langs {
        assert!(
            adapter_for(lang).is_some(),
            "language enum entry `{lang}` has no adapter"
        );
    }
}
