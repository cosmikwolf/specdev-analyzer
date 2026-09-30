//! Attribute-argument spike: can attribute arguments be read as data?
//!
//! The harness below is generic: it reads a query and a capture ->
//! attribute mapping from `tests/fixtures/attribute_args/unit/`, runs the query
//! over a source file, and compares the captured values with the matching
//! entities in module-spec TOML files, keyed by the entity's name. It
//! holds no unit-specific names; those live in the data files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

type Values = BTreeMap<String, BTreeMap<String, String>>; // key -> attr -> value

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/attribute_args")
}

struct Mapping {
    query: String,
    entity_table: String,
    spec_key: String,
    key_capture: String,
    arg_key_capture: String,
    arg_value_capture: String,
    attributes: BTreeMap<String, String>, // source arg -> spec attr
}

fn load_mapping() -> Mapping {
    let dir = fixture().join("unit");
    let t: toml::Table = std::fs::read_to_string(dir.join("mapping.toml")).unwrap().parse().unwrap();
    let s = |k: &str| t[k].as_str().unwrap().to_string();
    let attributes = t["attributes"]
        .as_table()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
        .collect();
    Mapping {
        query: std::fs::read_to_string(dir.join(s("query"))).unwrap(),
        entity_table: s("entity_table"),
        spec_key: s("spec_key"),
        key_capture: s("key_capture"),
        arg_key_capture: s("arg_key_capture"),
        arg_value_capture: s("arg_value_capture"),
        attributes,
    }
}

/// Code side: run the mapping's query; group values by entity key.
fn from_code(m: &Mapping, source: &str) -> Values {
    let lang: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let mut parser = Parser::new();
    parser.set_language(&lang).unwrap();
    let tree = parser.parse(source, None).unwrap();
    let query = Query::new(&lang, &m.query).expect("query compiles");
    let idx = |n: &str| query.capture_index_for_name(n).unwrap_or_else(|| panic!("capture @{n}"));
    let (ki, ai, vi) = (idx(&m.key_capture), idx(&m.arg_key_capture), idx(&m.arg_value_capture));
    let mut out = Values::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(mm) = matches.next() {
        let text = |i: u32| {
            mm.captures.iter().find(|c| c.index == i).map(|c| source[c.node.byte_range()].to_string())
        };
        let (Some(key), Some(arg), Some(val)) = (text(ki), text(ai), text(vi)) else { continue };
        let entry = out.entry(key).or_default();
        if let Some(attr) = m.attributes.get(&arg) {
            entry.insert(attr.clone(), val);
        }
    }
    out
}

/// Spec side: collect the mapped attributes of every entity in `entity_table`.
fn from_spec(m: &Mapping, specs: &[&str]) -> Values {
    let mut out = Values::new();
    for text in specs {
        let doc: toml::Table = text.parse().unwrap();
        let mut node = toml::Value::Table(doc);
        for part in m.entity_table.split('.') {
            node = match node.get(part) { Some(v) => v.clone(), None => toml::Value::Array(vec![]) };
        }
        for ent in node.as_array().unwrap() {
            let key = ent[&m.spec_key].as_str().unwrap().to_string();
            let entry = out.entry(key).or_default();
            for attr in m.attributes.values() {
                if let Some(v) = ent.get(attr) {
                    let s = match v { toml::Value::String(s) => s.clone(), other => other.to_string() };
                    entry.insert(attr.clone(), s);
                }
            }
        }
    }
    out
}

/// Every disagreement, as `key.attr: spec=<v> code=<v>` (missing = "-").
fn compare(spec: &Values, code: &Values) -> Vec<String> {
    let mut keys: Vec<&String> = spec.keys().chain(code.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut out = vec![];
    for k in keys {
        let (s, c) = (spec.get(k), code.get(k));
        let mut attrs: Vec<&String> = s.into_iter().flat_map(|m| m.keys()).chain(c.into_iter().flat_map(|m| m.keys())).collect();
        attrs.sort();
        attrs.dedup();
        if s.is_none() || c.is_none() {
            out.push(format!("{k}: spec={} code={}", s.is_some(), c.is_some()));
            continue;
        }
        for a in attrs {
            let (sv, cv) = (s.unwrap().get(a), c.unwrap().get(a));
            if sv != cv {
                out.push(format!("{k}.{a}: spec={} code={}", sv.map_or("-", |v| v), cv.map_or("-", |v| v)));
            }
        }
    }
    out
}

fn corpus() -> (String, Vec<String>) {
    let d = fixture().join("corpus");
    let src = std::fs::read_to_string(d.join("main.rs")).unwrap();
    let specs = ["Ticker.toml", "Blinky.toml"].iter().map(|f| std::fs::read_to_string(d.join(f)).unwrap()).collect();
    (src, specs)
}

fn run(src: &str, specs: &[String]) -> (Values, Vec<String>) {
    let m = load_mapping();
    let code = from_code(&m, src);
    let refs: Vec<&str> = specs.iter().map(String::as_str).collect();
    let spec = from_spec(&m, &refs);
    let diffs = compare(&spec, &code);
    (code, diffs)
}

// Exit rule 1: each task's name, `binds` (when present) and `priority`
// come out as separate values.
#[test]
fn extraction_yields_separate_values_per_task() {
    let (src, specs) = corpus();
    let (code, _) = run(&src, &specs);
    let expect = |k: &str, pairs: &[(&str, &str)]| {
        let got = code.get(k).unwrap_or_else(|| panic!("no captures for {k}: {code:?}"));
        let want: BTreeMap<String, String> = pairs.iter().map(|(a, v)| (a.to_string(), v.to_string())).collect();
        assert_eq!(got, &want, "{k}");
    };
    assert_eq!(code.len(), 3, "exactly the three tasks: {code:?}");
    expect("on_tick", &[("binds", "PIT0"), ("priority", "3")]);
    expect("drain", &[("priority", "1")]);
    expect("blink", &[("priority", "1")]);
}

// Exit rule 2: the data mapping compares code with spec by entity key; all agree.
#[test]
fn mapping_as_data_matches_the_spec() {
    let (src, specs) = corpus();
    let (_, diffs) = run(&src, &specs);
    assert!(diffs.is_empty(), "unexpected disagreements: {diffs:?}");
}

// Exit rule 3: negative controls — each changed value is reported, and only it.
#[test]
fn a_changed_priority_is_reported_exactly() {
    let (src, specs) = corpus();
    let changed = src.replacen("#[task(shared = [count], priority = 1)]", "#[task(shared = [count], priority = 2)]", 1);
    assert_ne!(changed, src, "the control must change the source");
    let (_, diffs) = run(&changed, &specs);
    assert_eq!(diffs, vec!["drain.priority: spec=1 code=2".to_string()]);
}

#[test]
fn a_changed_binding_is_reported_exactly() {
    let (src, specs) = corpus();
    let changed = src.replacen("binds = PIT0,", "binds = PIT1,", 1);
    assert_ne!(changed, src, "the control must change the source");
    let (_, diffs) = run(&changed, &specs);
    assert_eq!(diffs, vec!["on_tick.binds: spec=PIT0 code=PIT1".to_string()]);
}

#[test]
fn a_task_missing_from_the_code_is_reported() {
    let (src, specs) = corpus();
    let changed = src.replacen("#[task(local = [led_on, status_led], priority = 1)]", "", 1);
    assert_ne!(changed, src, "the control must change the source");
    let (_, diffs) = run(&changed, &specs);
    assert_eq!(diffs, vec!["blink: spec=true code=false".to_string()]);
}
