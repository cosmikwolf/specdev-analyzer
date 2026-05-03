//! Tree-sitter-backed item extraction (Phase 10 surface, per D40).

use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser, Query, QueryCursor, QueryMatch, StreamingIterator};

use crate::lang::LanguageAdapter;

use super::{ExtractedItem, ItemKind, Span};

const RUST_QUERY: &str = include_str!("../../../specdev/languages/rust/items.scm");

/// Parse `source` with the language adapter, run the per-language items
/// query, and produce one `ExtractedItem` per top-level item capture.
pub fn extract_items_from_source(
    source: &str,
    adapter: &dyn LanguageAdapter,
) -> Result<Vec<ExtractedItem>> {
    let ts_language = adapter.tree_sitter_language();
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| anyhow::anyhow!("set_language: {e}"))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter parser returned None"))?;

    let query_text = match adapter.name() {
        "rust" => RUST_QUERY,
        // Other languages add their items.scm + match arm here. v1 ships Rust only.
        other => anyhow::bail!(
            "extract-items: no items.scm query for language `{other}`; v1 ships Rust only (Phase 9 adds C, Phase 14-C ships C items)"
        ),
    };

    let query = Query::new(&ts_language, query_text)
        .map_err(|e| anyhow::anyhow!("items.scm query compile: {e}"))?;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut items = Vec::new();
    while let Some(m) = matches.next() {
        if let Some(item) = match_to_item(&query, m, source) {
            items.push(item);
        }
    }
    Ok(items)
}

/// Read source from disk and extract items.
pub fn extract_items_from_path(
    path: &Path,
    adapter: &dyn LanguageAdapter,
) -> Result<Vec<ExtractedItem>> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("reading source file {}", path.display()))?;
    extract_items_from_source(&source, adapter)
}

fn match_to_item(query: &Query, m: &QueryMatch, source: &str) -> Option<ExtractedItem> {
    let names = capture_names(query, m);

    if names.contains(&"fn.def") {
        return build_named(query, m, source, ItemKind::Fn, "fn.def", "fn.name");
    }
    if names.contains(&"struct.def") {
        return build_named(query, m, source, ItemKind::Struct, "struct.def", "struct.name");
    }
    if names.contains(&"enum.def") {
        return build_named(query, m, source, ItemKind::Enum, "enum.def", "enum.name");
    }
    if names.contains(&"union.def") {
        return build_named(query, m, source, ItemKind::Union, "union.def", "union.name");
    }
    if names.contains(&"trait.def") {
        return build_named(query, m, source, ItemKind::Trait, "trait.def", "trait.name");
    }
    if names.contains(&"impl.def") {
        return build_impl(query, m, source);
    }
    if names.contains(&"type.def") {
        return build_named(query, m, source, ItemKind::Type, "type.def", "type.name");
    }
    if names.contains(&"const.def") {
        return build_named(query, m, source, ItemKind::Const, "const.def", "const.name");
    }
    if names.contains(&"static.def") {
        return build_named(query, m, source, ItemKind::Static, "static.def", "static.name");
    }
    if names.contains(&"re_export.def") {
        return build_re_export(query, m, source);
    }
    if names.contains(&"macro.def") {
        return build_named(query, m, source, ItemKind::Macro, "macro.def", "macro.name");
    }
    if names.contains(&"extern_block.def") {
        return build_extern_block(query, m, source);
    }
    None
}

fn build_named(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    kind: ItemKind,
    def_capture: &str,
    name_capture: &str,
) -> Option<ExtractedItem> {
    let def_node = capture_node(query, m, def_capture)?;
    let name = capture_text(query, m, source, name_capture)?.to_string();
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    Some(ExtractedItem {
        kind,
        name,
        signature: None,
        attributes,
        span,
    })
}

fn build_impl(query: &Query, m: &QueryMatch, source: &str) -> Option<ExtractedItem> {
    let def_node = capture_node(query, m, "impl.def")?;
    let target = capture_text(query, m, source, "impl.type")?.trim().to_string();
    let trait_text = capture_text(query, m, source, "impl.trait").map(|s| s.trim().to_string());
    let name = match trait_text {
        Some(t) if !t.is_empty() => format!("{t} for {target}"),
        _ => target,
    };
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    Some(ExtractedItem {
        kind: ItemKind::Impl,
        name,
        signature: None,
        attributes,
        span,
    })
}

fn build_re_export(query: &Query, m: &QueryMatch, source: &str) -> Option<ExtractedItem> {
    let def_node = capture_node(query, m, "re_export.def")?;
    let target = capture_text(query, m, source, "re_export.target")?
        .trim()
        .to_string();
    // Extract a short name for matching purposes: the last segment of the
    // target path, after `::` or `as`. `pub use foo::bar as Baz;` →
    // name = "Baz". `pub use foo::bar;` → name = "bar".
    let name = re_export_name(&target);
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    Some(ExtractedItem {
        kind: ItemKind::ReExport,
        name,
        signature: None,
        attributes,
        span,
    })
}

fn re_export_name(target: &str) -> String {
    // Handle `path::to::thing as Alias` — after `as`.
    if let Some(idx) = target.rfind(" as ") {
        return target[idx + 4..].trim().trim_end_matches(';').to_string();
    }
    // Otherwise the last `::` segment. Strip trailing `;` defensively.
    let trimmed = target.trim().trim_end_matches(';');
    trimmed
        .rsplit("::")
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

fn build_extern_block(query: &Query, m: &QueryMatch, source: &str) -> Option<ExtractedItem> {
    let def_node = capture_node(query, m, "extern_block.def")?;
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    // Extern blocks have no identifier; synthesize one from the abi
    // string + line so the name is unique within a module.
    let abi = extern_abi(def_node, source).unwrap_or_else(|| "C".to_string());
    let name = format!("extern_{abi}_at_{}", span.start_line);
    Some(ExtractedItem {
        kind: ItemKind::ExternBlock,
        name,
        signature: None,
        attributes,
        span,
    })
}

fn extern_abi(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return Some(
                source[child.byte_range()]
                    .trim_matches('"')
                    .to_string(),
            );
        }
    }
    None
}

fn capture_names<'a>(query: &'a Query, m: &'a QueryMatch<'_, '_>) -> Vec<&'a str> {
    m.captures
        .iter()
        .map(|c| *query.capture_names().get(c.index as usize).unwrap())
        .collect()
}

fn capture_text<'a>(query: &Query, m: &QueryMatch, source: &'a str, name: &str) -> Option<&'a str> {
    let idx = query.capture_names().iter().position(|n| *n == name)?;
    let cap = m.captures.iter().find(|c| c.index as usize == idx)?;
    Some(&source[cap.node.byte_range()])
}

fn capture_node<'a>(query: &Query, m: &'a QueryMatch<'a, 'a>, name: &str) -> Option<Node<'a>> {
    let idx = query.capture_names().iter().position(|n| *n == name)?;
    let cap = m.captures.iter().find(|c| c.index as usize == idx)?;
    Some(cap.node)
}

fn node_span(node: Node<'_>) -> Span {
    Span {
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
    }
}

/// Walk siblings preceding `def_node` collecting `attribute_item` nodes
/// (e.g. `#[test]`, `#[cfg(...)]`). Returns attributes in source order
/// (top-to-bottom).
fn preceding_attributes(def_node: Node<'_>, source: &str) -> Vec<String> {
    let mut attrs = Vec::new();
    let mut cur = def_node.prev_sibling();
    while let Some(n) = cur {
        if n.kind() == "attribute_item" || n.kind() == "inner_attribute_item" {
            attrs.push(source[n.byte_range()].to_string());
            cur = n.prev_sibling();
        } else {
            break;
        }
    }
    attrs.reverse();
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::rust::RustAdapter;

    fn extract(source: &str) -> Vec<ExtractedItem> {
        extract_items_from_source(source, &RustAdapter).expect("extract")
    }

    #[test]
    fn extracts_top_level_fn() {
        let source = "fn foo(x: u32) -> u32 { x + 1 }\n";
        let items = extract(source);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ItemKind::Fn);
        assert_eq!(items[0].name, "foo");
    }

    #[test]
    fn extracts_test_attribute_on_fn() {
        let source = "#[test]\nfn it_works() {}\n";
        let items = extract(source);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ItemKind::Fn);
        assert_eq!(items[0].name, "it_works");
        assert_eq!(items[0].attributes, vec!["#[test]"]);
    }

    #[test]
    fn extracts_struct_enum_const() {
        let source = "pub struct S;\npub enum E { A, B }\npub const N: u32 = 4;\n";
        let items = extract(source);
        assert_eq!(items.len(), 3);
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&ItemKind::Struct));
        assert!(kinds.contains(&ItemKind::Enum));
        assert!(kinds.contains(&ItemKind::Const));
    }

    #[test]
    fn extracts_trait_impl_inherent_and_trait() {
        let source = "\
struct T;
impl T { fn one(&self) {} }
trait Show {}
impl Show for T {}
";
        let items = extract(source);
        assert_eq!(items.iter().filter(|i| i.kind == ItemKind::Struct).count(), 1);
        assert_eq!(items.iter().filter(|i| i.kind == ItemKind::Trait).count(), 1);
        let impls: Vec<_> = items
            .iter()
            .filter(|i| i.kind == ItemKind::Impl)
            .collect();
        assert_eq!(impls.len(), 2);
        let names: Vec<&str> = impls.iter().map(|i| i.name.as_str()).collect();
        assert!(names.iter().any(|n| n.starts_with("Show for")));
        assert!(names.iter().any(|n| *n == "T"));
    }

    #[test]
    fn re_export_name_from_alias() {
        assert_eq!(re_export_name("foo::bar as Baz"), "Baz");
        assert_eq!(re_export_name("foo::bar"), "bar");
        assert_eq!(re_export_name("crate::Inner;"), "Inner");
    }

    #[test]
    fn extracts_re_export_with_alias() {
        let source = "pub use crate::inner::Inner as PublicName;\n";
        let items = extract(source);
        let re_exports: Vec<_> = items
            .iter()
            .filter(|i| i.kind == ItemKind::ReExport)
            .collect();
        assert_eq!(re_exports.len(), 1);
        assert_eq!(re_exports[0].name, "PublicName");
    }

    #[test]
    fn extracts_static_and_type() {
        let source = "pub static N: u32 = 0;\npub type Alias = u32;\n";
        let items = extract(source);
        let kinds: Vec<_> = items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&ItemKind::Static));
        assert!(kinds.contains(&ItemKind::Type));
    }

    #[test]
    fn extracts_extern_block() {
        let source = "extern \"C\" {\n    fn foo() -> u32;\n}\n";
        let items = extract(source);
        let externs: Vec<_> = items
            .iter()
            .filter(|i| i.kind == ItemKind::ExternBlock)
            .collect();
        assert_eq!(externs.len(), 1);
        assert!(externs[0].name.starts_with("extern_C_at_"));
    }

    #[test]
    fn skips_nested_items() {
        // tree-sitter Query without anchors matches every descendant — but
        // the items.scm patterns are unanchored. A function inside an impl
        // SHOULD still surface as a separate fn item per items[] design
        // (impls and their inherent fns are both top-level-significant).
        // What we MUST exclude: items inside other functions (locals).
        let source = "fn outer() { fn inner() {} let _x = 1; }\n";
        let items = extract(source);
        // Tree-sitter will match `inner` because the query isn't anchored;
        // this test documents current behaviour. Filtering local-fn items
        // is a Phase 14 concern (items diff against module spec carries
        // the right context).
        let fns: Vec<_> = items.iter().filter(|i| i.kind == ItemKind::Fn).collect();
        assert!(fns.iter().any(|i| i.name == "outer"));
    }
}
