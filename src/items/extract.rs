//! Tree-sitter-backed item extraction.

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
            "extract-items: no items.scm query for language `{other}`; v1 ships Rust only"
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
        return build_free_fn(query, m, source);
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
    // Inherent-impl associated items: each public member is an item named
    // `<Type>::<member>`. The inherent impl block itself is not an item.
    if names.contains(&"inherent_fn.def") {
        return build_inherent_assoc(
            query, m, source, ItemKind::Fn, "inherent_fn.def", "inherent_fn.name", "inherent_fn.vis",
        );
    }
    if names.contains(&"inherent_const.def") {
        return build_inherent_assoc(
            query,
            m,
            source,
            ItemKind::Const,
            "inherent_const.def",
            "inherent_const.name",
            "inherent_const.vis",
        );
    }
    if names.contains(&"inherent_type.def") {
        return build_inherent_assoc(
            query,
            m,
            source,
            ItemKind::Type,
            "inherent_type.def",
            "inherent_type.name",
            "inherent_type.vis",
        );
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

/// Build an item for a top-level free function, filtered to `pub`.
/// A non-`pub` (or `pub(crate)`/`pub(super)`) free fn is a module-private
/// helper, not public contract surface, so it must not surface as
/// `in_source_only` drift — the conventions say private/helper members
/// aren't items. Mirrors the inherent-method visibility filter in
/// [`build_inherent_assoc`].
fn build_free_fn(query: &Query, m: &QueryMatch, source: &str) -> Option<ExtractedItem> {
    let vis = capture_text(query, m, source, "fn.vis").map(str::trim);
    if vis != Some("pub") {
        return None;
    }
    build_named(query, m, source, ItemKind::Fn, "fn.def", "fn.name")
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

/// Build an item for a public associated member of an **inherent** impl.
/// The item is named `<TypeBase>::<member>` (generics + path qualifiers
/// stripped from the host type so the name matches what an author writes).
/// Returns `None` for non-`pub` members — private associated items are not
/// contract surface and must not surface as `in_source_only` drift.
fn build_inherent_assoc(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    kind: ItemKind,
    def_capture: &str,
    name_capture: &str,
    vis_capture: &str,
) -> Option<ExtractedItem> {
    // Contract surface is `pub` only (mirrors the top-level "helpers are
    // bare" rule). `pub(crate)`/`pub(super)` are not public API.
    let vis = capture_text(query, m, source, vis_capture).map(str::trim);
    if vis != Some("pub") {
        return None;
    }
    let def_node = capture_node(query, m, def_capture)?;
    let host = capture_text(query, m, source, "inherent.host")?;
    let member = capture_text(query, m, source, name_capture)?;
    let name = format!("{}::{member}", super::sync::type_base(host));
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
        let source = "pub fn foo(x: u32) -> u32 { x + 1 }\n";
        let items = extract(source);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, ItemKind::Fn);
        assert_eq!(items[0].name, "foo");
    }

    // A non-`pub` top-level free fn is a module-private helper, not
    // contract surface, so it is filtered out (R-002).
    #[test]
    fn private_top_level_free_fn_is_not_an_item() {
        let source = "fn make_bucket() -> u32 { 0 }\npub fn render() -> u32 { 1 }\n";
        let items = extract(source);
        let fns: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == ItemKind::Fn)
            .map(|i| i.name.as_str())
            .collect();
        assert_eq!(fns, vec!["render"], "private free fn must be filtered");
    }

    // `pub(crate)` / `pub(super)` are not public API either.
    #[test]
    fn pub_crate_free_fn_is_not_an_item() {
        let source = "pub(crate) fn internal() {}\npub(super) fn up() {}\n";
        let items = extract(source);
        assert!(
            !items.iter().any(|i| i.kind == ItemKind::Fn),
            "restricted-visibility free fns must be filtered"
        );
    }

    #[test]
    fn extracts_test_attribute_on_fn() {
        // Attribute capture is visibility-independent; use a `pub` fn so the
        // R-002 free-fn filter keeps it (the capture mechanism is what's
        // under test, not realistic test-fn placement).
        let source = "#[test]\npub fn it_works() {}\n";
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

    // Inherent impl dissolves into per-method items; a trait impl stays one
    // item whose methods are codegen-derived (not surfaced).
    #[test]
    fn inherent_impl_dissolves_trait_impl_stays_one_item() {
        let source = "\
struct T;
impl T { pub fn one(&self) {} fn helper(&self) {} }
trait Show {}
impl Show for T { fn show(&self) {} }
";
        let items = extract(source);
        // The inherent impl produces no impl item; its public fn is `T::one`,
        // its private `helper` is not an item.
        assert!(
            !items.iter().any(|i| i.kind == ItemKind::Impl && i.name == "T"),
            "inherent impl block must not be an item"
        );
        assert!(
            items.iter().any(|i| i.kind == ItemKind::Fn && i.name == "T::one"),
            "public inherent method must extract as `T::one`"
        );
        assert!(
            !items.iter().any(|i| i.name == "T::helper"),
            "private inherent method must not be an item"
        );
        // The trait impl stays exactly one item; its method is not surfaced.
        let impls: Vec<_> = items.iter().filter(|i| i.kind == ItemKind::Impl).collect();
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "Show for T");
        assert!(
            !items.iter().any(|i| i.name.contains("show")),
            "trait-impl method bodies are codegen-derived, not items"
        );
    }

    // Generics on the host type are normalized out of the qualified name
    // (no `RateLimiter` vs `RateLimiter<C>` split).
    #[test]
    fn inherent_method_name_normalizes_host_generics() {
        let source = "impl RateLimiter<C> { pub fn try_consume(&self, n: u32) -> bool { true } }\n";
        let items = extract(source);
        assert!(
            items.iter().any(|i| i.kind == ItemKind::Fn && i.name == "RateLimiter::try_consume"),
            "expected `RateLimiter::try_consume`, got {:?}",
            items.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    // Public associated const / type in an inherent impl are items.
    #[test]
    fn inherent_assoc_const_and_type() {
        let source = "impl Widget { pub const MAX: u32 = 8; pub type Out = u16; fn p() {} }\n";
        let items = extract(source);
        assert!(items.iter().any(|i| i.kind == ItemKind::Const && i.name == "Widget::MAX"));
        assert!(items.iter().any(|i| i.kind == ItemKind::Type && i.name == "Widget::Out"));
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
    fn skips_function_body_locals() {
        // Patterns are anchored to `source_file` (top level) plus inherent
        // `impl` bodies. A fn nested inside another fn's body is neither, so
        // only `outer` is extracted — `inner` is a local.
        let source = "pub fn outer() { fn inner() {} let _x = 1; }\n";
        let items = extract(source);
        let fns: Vec<_> = items.iter().filter(|i| i.kind == ItemKind::Fn).collect();
        assert!(fns.iter().any(|i| i.name == "outer"));
        assert!(
            !fns.iter().any(|i| i.name == "inner"),
            "function-body locals must not be items"
        );
    }
}
