//! Tree-sitter-backed source parser, shared across language adapters.
//!
//! The per-language specifics live behind the [`LanguageAdapter`] trait;
//! this module walks query matches and hydrates [`CodeConstruct`] values.

use tree_sitter::{Node, Query, QueryCursor, QueryMatch, StreamingIterator};

use crate::lang::{LanguageAdapter, ParsedParameter};

use super::construct::{CodeConstruct, ConstructKind, TraitMethod};
use super::AuditError;

/// Parse `source` using the supplied adapter. `query_source` allows the
/// caller to override the adapter's embedded query (e.g., via a plugin's
/// `languages/<name>/audit.scm` file).
pub fn parse_source(
    source: &str,
    adapter: &dyn LanguageAdapter,
    query_source: Option<&str>,
) -> Result<Vec<CodeConstruct>, AuditError> {
    let ts_language = adapter.tree_sitter_language();

    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| AuditError::TreeSitter(format!("set_language: {e}")))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AuditError::TreeSitter("parser returned None".into()))?;

    let query_text = query_source.unwrap_or_else(|| adapter.embedded_query());
    let query = Query::new(&ts_language, query_text)
        .map_err(|e| AuditError::TreeSitter(format!("query compile: {e}")))?;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut constructs = Vec::new();
    while let Some(m) = matches.next() {
        if let Some(c) = match_to_construct(&query, m, source, adapter) {
            constructs.push(c);
        }
    }
    Ok(constructs)
}

fn capture_names<'a>(query: &'a Query, m: &'a QueryMatch<'_, '_>) -> Vec<&'a str> {
    m.captures
        .iter()
        .map(|c| *query.capture_names().get(c.index as usize).unwrap())
        .collect()
}

fn match_to_construct(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    adapter: &dyn LanguageAdapter,
) -> Option<CodeConstruct> {
    let names = capture_names(query, m);

    if names.contains(&"fn.def") {
        return parse_function(query, m, source, adapter, "fn");
    }
    if names.contains(&"method.def") {
        return parse_function(query, m, source, adapter, "method");
    }
    if names.contains(&"struct.def") {
        return parse_struct(query, m, source);
    }
    if names.contains(&"enum.def") {
        return parse_enum(query, m, source);
    }
    if names.contains(&"impl.def") {
        return parse_impl(query, m, source);
    }
    if names.contains(&"trait.def") {
        return parse_trait(query, m, source, adapter);
    }
    if names.contains(&"typedef.def") {
        return parse_typedef(query, m, source);
    }
    None
}

fn capture_text<'a>(
    query: &Query,
    m: &QueryMatch,
    source: &'a str,
    name: &str,
) -> Option<&'a str> {
    let idx = query.capture_names().iter().position(|n| *n == name)?;
    let cap = m.captures.iter().find(|c| c.index as usize == idx)?;
    Some(&source[cap.node.byte_range()])
}

fn capture_node<'a>(
    query: &'a Query,
    m: &'a QueryMatch<'a, 'a>,
    name: &str,
) -> Option<Node<'a>> {
    let idx = query.capture_names().iter().position(|n| *n == name)?;
    let cap = m.captures.iter().find(|c| c.index as usize == idx)?;
    Some(cap.node)
}

fn parse_function(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    adapter: &dyn LanguageAdapter,
    prefix: &str,
) -> Option<CodeConstruct> {
    let name = capture_text(query, m, source, &format!("{prefix}.name"))?.to_string();
    let params_node = capture_node(query, m, &format!("{prefix}.params"))?;
    let line = params_node.start_position().row + 1;
    let parameters = parse_parameter_list(params_node, source, adapter);

    Some(CodeConstruct {
        kind: ConstructKind::Function,
        name,
        parameters,
        fields: Vec::new(),
        variants: Vec::new(),
        methods: Vec::new(),
        line,
    })
}

fn parse_struct(query: &Query, m: &QueryMatch, source: &str) -> Option<CodeConstruct> {
    let name = capture_text(query, m, source, "struct.name")?.to_string();
    let def = capture_node(query, m, "struct.def")?;
    let line = def.start_position().row + 1;

    let fields = match capture_node(query, m, "struct.fields") {
        Some(n) => extract_field_names(n, source),
        None => Vec::new(),
    };

    Some(CodeConstruct {
        kind: ConstructKind::Struct,
        name,
        parameters: Vec::new(),
        fields,
        variants: Vec::new(),
        methods: Vec::new(),
        line,
    })
}

fn parse_enum(query: &Query, m: &QueryMatch, source: &str) -> Option<CodeConstruct> {
    let name = capture_text(query, m, source, "enum.name")?.to_string();
    let def = capture_node(query, m, "enum.def")?;
    let line = def.start_position().row + 1;

    let variants = match capture_node(query, m, "enum.variants") {
        Some(n) => extract_variant_names(n, source),
        None => Vec::new(),
    };

    Some(CodeConstruct {
        kind: ConstructKind::Enum,
        name,
        parameters: Vec::new(),
        fields: Vec::new(),
        variants,
        methods: Vec::new(),
        line,
    })
}

fn parse_impl(query: &Query, m: &QueryMatch, source: &str) -> Option<CodeConstruct> {
    let type_text = capture_text(query, m, source, "impl.type")?.to_string();
    let def = capture_node(query, m, "impl.def")?;
    let line = def.start_position().row + 1;

    Some(CodeConstruct {
        kind: ConstructKind::ImplBlock,
        name: type_text,
        parameters: Vec::new(),
        fields: Vec::new(),
        variants: Vec::new(),
        methods: Vec::new(),
        line,
    })
}

fn parse_trait(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    adapter: &dyn LanguageAdapter,
) -> Option<CodeConstruct> {
    let name = capture_text(query, m, source, "trait.name")?.to_string();
    let def = capture_node(query, m, "trait.def")?;
    let line = def.start_position().row + 1;

    let methods = match capture_node(query, m, "trait.body") {
        Some(n) => extract_trait_methods(n, source, adapter),
        None => Vec::new(),
    };

    Some(CodeConstruct {
        kind: ConstructKind::Trait,
        name,
        parameters: Vec::new(),
        fields: Vec::new(),
        variants: Vec::new(),
        methods,
        line,
    })
}

/// C `typedef struct {...} Name;` — surfaced as a Struct keyed by the
/// typedef name so audit can match modules whose name is the typedef.
fn parse_typedef(query: &Query, m: &QueryMatch, source: &str) -> Option<CodeConstruct> {
    let typedef_name = capture_text(query, m, source, "typedef.name")?.to_string();
    let def = capture_node(query, m, "typedef.def")?;
    let line = def.start_position().row + 1;
    let fields = match capture_node(query, m, "typedef.struct_fields") {
        Some(n) => extract_field_names(n, source),
        None => Vec::new(),
    };

    Some(CodeConstruct {
        kind: ConstructKind::Struct,
        name: typedef_name,
        parameters: Vec::new(),
        fields,
        variants: Vec::new(),
        methods: Vec::new(),
        line,
    })
}

fn parse_parameter_list(
    params_node: Node,
    source: &str,
    adapter: &dyn LanguageAdapter,
) -> Vec<ParsedParameter> {
    let mut params = Vec::new();
    let mut cursor = params_node.walk();
    let kind = adapter.parameter_node_kind();
    let self_kind = adapter.self_node_kind();

    for child in params_node.children(&mut cursor) {
        if child.kind() == kind {
            if let Some(p) = adapter.parse_parameter(child, source) {
                params.push(p);
            }
        } else if Some(child.kind()) == self_kind {
            if let Some(p) = adapter.parse_self_parameter(child, source) {
                params.push(p);
            }
        }
    }
    params
}

fn extract_field_names(node: Node, source: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "field_declaration" {
            let mut inner = child.walk();
            for fc in child.children(&mut inner) {
                if fc.kind() == "field_identifier" {
                    fields.push(source[fc.byte_range()].to_string());
                }
            }
        }
    }
    fields
}

fn extract_variant_names(node: Node, source: &str) -> Vec<String> {
    let mut variants = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "enum_variant" => {
                let mut inner = child.walk();
                for vc in child.children(&mut inner) {
                    if vc.kind() == "identifier" {
                        variants.push(source[vc.byte_range()].to_string());
                    }
                }
            }
            "enumerator" => {
                let mut inner = child.walk();
                for vc in child.children(&mut inner) {
                    if vc.kind() == "identifier" {
                        variants.push(source[vc.byte_range()].to_string());
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    variants
}

fn extract_trait_methods(
    node: Node,
    source: &str,
    adapter: &dyn LanguageAdapter,
) -> Vec<TraitMethod> {
    let mut methods = Vec::new();
    let mut cursor = node.walk();

    for child in node.children(&mut cursor) {
        if child.kind() == "function_signature_item" || child.kind() == "function_item" {
            let mut name = String::new();
            let mut parameters = Vec::new();
            let mut inner = child.walk();
            for sc in child.children(&mut inner) {
                match sc.kind() {
                    "identifier" if name.is_empty() => {
                        name = source[sc.byte_range()].to_string();
                    }
                    "parameters" => {
                        parameters = parse_parameter_list(sc, source, adapter);
                    }
                    _ => {}
                }
            }
            if !name.is_empty() {
                methods.push(TraitMethod { name, parameters });
            }
        }
    }
    methods
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang;

    fn parse(source: &str, language: &str) -> Vec<CodeConstruct> {
        let adapter = lang::adapter_for(language).expect("adapter");
        parse_source(source, adapter.as_ref(), None).expect("parse")
    }

    #[test]
    fn rust_function_and_struct() {
        let src = r#"
pub struct StatusLed { pin: u8 }

pub fn init(pin: u8) -> StatusLed { StatusLed { pin } }
"#;
        let out = parse(src, "rust");
        let fns: Vec<_> = out.iter().filter(|c| c.kind == ConstructKind::Function).collect();
        let structs: Vec<_> = out.iter().filter(|c| c.kind == ConstructKind::Struct).collect();
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].name, "init");
        assert_eq!(fns[0].parameters[0].name, "pin");
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "StatusLed");
    }

    #[test]
    fn rust_enum_and_trait() {
        let src = r#"
pub enum Mode { Off, On, Dim }

pub trait Port {
    fn send(&self, byte: u8);
}
"#;
        let out = parse(src, "rust");
        let e = out.iter().find(|c| c.kind == ConstructKind::Enum).unwrap();
        assert_eq!(e.name, "Mode");
        assert_eq!(e.variants, vec!["Off", "On", "Dim"]);

        let t = out.iter().find(|c| c.kind == ConstructKind::Trait).unwrap();
        assert_eq!(t.name, "Port");
        assert_eq!(t.methods.len(), 1);
        assert_eq!(t.methods[0].name, "send");
    }

    #[test]
    fn rust_never_panics_on_garbage() {
        let junk = "}}}fn struct enum {{{ 12345 !!! @@@";
        let _ = parse(junk, "rust");
    }

    #[test]
    fn c_function_and_struct_typedef() {
        let src = r#"
typedef struct {
    int pin;
} StatusLed;

void status_led_init(int pin) {}
"#;
        let out = parse(src, "c");
        let fns: Vec<_> = out.iter().filter(|c| c.kind == ConstructKind::Function).collect();
        let structs: Vec<_> = out.iter().filter(|c| c.kind == ConstructKind::Struct).collect();
        assert_eq!(fns.len(), 1);
        assert_eq!(fns[0].name, "status_led_init");
        assert!(
            structs.iter().any(|s| s.name == "StatusLed"),
            "expected typedef-struct StatusLed: {structs:?}"
        );
    }

    #[test]
    fn cpp_class_and_method() {
        let src = r#"
class StatusLed {
public:
    void init(int pin) {}
    int read() { return 0; }
};

void free_fn() {}
"#;
        let out = parse(src, "cpp");
        let structs: Vec<_> = out.iter().filter(|c| c.kind == ConstructKind::Struct).collect();
        let fns: Vec<_> = out.iter().filter(|c| c.kind == ConstructKind::Function).collect();
        assert!(structs.iter().any(|s| s.name == "StatusLed"));
        let names: Vec<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"free_fn"), "fns={names:?}");
        assert!(names.contains(&"init"), "fns={names:?}");
        assert!(names.contains(&"read"), "fns={names:?}");
    }

    #[test]
    fn cpp_enum_class() {
        let src = r#"
enum class Mode { Off, On, Dim };
"#;
        let out = parse(src, "cpp");
        let e = out.iter().find(|c| c.kind == ConstructKind::Enum).unwrap();
        assert_eq!(e.name, "Mode");
        assert_eq!(e.variants, vec!["Off", "On", "Dim"]);
    }

    #[test]
    fn unknown_language_returns_none() {
        assert!(lang::adapter_for("fortran").is_none());
    }
}
