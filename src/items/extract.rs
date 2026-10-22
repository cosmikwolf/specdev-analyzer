//! Tree-sitter-backed item extraction.
//!
//! Dispatch is capture-name-driven: each language's `items.scm` names its
//! captures `<kind>.def` / `<kind>.name` (with `<kind>.vis` where the
//! grammar has visibility modifiers), and the capture prefix becomes the
//! extracted item's `kind` string verbatim — the kind vocabulary lives in
//! the language tier, not here. Members of a type use the
//! `inherent_<kind>.def` / `inherent_<kind>.name` + `inherent.host`
//! convention and extract as `<host><sep><member>` with the adapter's
//! path separator. Three capture prefixes keep dedicated builders for
//! their synthesized names (`impl`, `re_export`, `extern_block` — Rust
//! conventions; a language that doesn't declare them never hits them).
//!
//! Visibility (#101(b)): a kind whose query declares `<kind>.vis` is
//! compared against a *floor* — the profile's `items.visibility_floor`
//! when declared, else the adapter's default. The floor is an opaque
//! string; only [`LanguageAdapter::visibility_at_least`] reads it, so no
//! visibility literal lives here. `re_export` is the exception: the spec's
//! own `visibility` enum decides per item, via
//! [`LanguageAdapter::re_export_visibility`] (#104).

use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser, Query, QueryCursor, QueryMatch, StreamingIterator};

use crate::lang::LanguageAdapter;
use crate::manifest::Profile;

use super::{ExtractedItem, Span};

/// Read the profile-declared visibility floor from the project's inlined
/// `profile.yaml` (`items.visibility_floor`). `Ok(None)` when the manifest
/// or the key is absent — the adapter default then applies.
pub fn profile_visibility_floor(specdev_root: &Path) -> Result<Option<String>> {
    Ok(Profile::load_inlined(specdev_root)?.and_then(|p| p.visibility_floor().map(str::to_string)))
}

/// Parse `source` with the language adapter, run the per-language items
/// query, and produce one `ExtractedItem` per top-level item capture.
/// `ext` is the source file's extension and selects the grammar — a
/// language may span several (`.ts` / `.tsx`), and `source_file` makes
/// the extension per-module data, so the caller passes what the resolved
/// path carries. `floor` is the profile-declared visibility floor; `None`
/// falls back to the adapter's default.
pub fn extract_items_from_source(
    source: &str,
    ext: &str,
    adapter: &dyn LanguageAdapter,
    floor: Option<&str>,
) -> Result<Vec<ExtractedItem>> {
    let floor = floor.or_else(|| adapter.default_visibility_floor());
    let ts_language = adapter.tree_sitter_language_for(ext);
    let mut parser = Parser::new();
    parser
        .set_language(&ts_language)
        .map_err(|e| anyhow::anyhow!("set_language: {e}"))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter parser returned None"))?;

    let query_text = adapter.items_query().ok_or_else(|| {
        anyhow::anyhow!(
            "extract-items: no items.scm query for language `{}`; the language \
             tier does not ship an items vocabulary",
            adapter.name()
        )
    })?;

    let query = Query::new(&ts_language, query_text)
        .map_err(|e| anyhow::anyhow!("items.scm query compile: {e}"))?;

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    let mut items = Vec::new();
    while let Some(m) = matches.next() {
        if let Some(item) = match_to_item(&query, m, source, adapter, floor) {
            items.push(item);
        }
    }
    Ok(items)
}

/// Read source from disk and extract items.
pub fn extract_items_from_path(
    path: &Path,
    adapter: &dyn LanguageAdapter,
    floor: Option<&str>,
) -> Result<Vec<ExtractedItem>> {
    let source = std::fs::read_to_string(path)
        .with_context(|| format!("reading source file {}", path.display()))?;
    // Grammar selection keys off the resolved path, not the adapter's
    // primary extension: `source_file` may name a `.tsx` for a `.ts` language.
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_else(|| adapter.file_extension());
    extract_items_from_source(&source, ext, adapter, floor)
}

fn match_to_item(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    adapter: &dyn LanguageAdapter,
    floor: Option<&str>,
) -> Option<ExtractedItem> {
    // Each query pattern carries exactly one `.def` capture; its prefix is
    // the kind (or `inherent_<kind>` for member items).
    let names = capture_names(query, m);
    let def = names.iter().find(|n| n.ends_with(".def"))?;
    let prefix = def.strip_suffix(".def").expect("checked ends_with");

    match prefix {
        "impl" => build_impl(query, m, source),
        "re_export" => build_re_export(query, m, source, adapter),
        "extern_block" => build_extern_block(query, m, source),
        p if p.starts_with("inherent_") => build_member(query, m, source, adapter, p, floor),
        p => build_named(query, m, source, adapter, p, floor),
    }
}

/// True when the match's visibility is at or above the floor. Three
/// early-outs: the query declares no `<prefix>.vis` capture for this kind
/// (Rust `macro`; every Dart kind) — no gate, the adapter's name rule is
/// the only filter; or the language has no floor at all (`None`) — same.
/// Otherwise a declared-but-absent capture means "no modifier written",
/// and the adapter decides whether that clears the floor (in Rust it never
/// does). `prefix` is the capture prefix (`fn`, `inherent_fn`, `struct`),
/// not the schema kind.
fn vis_gate_passes(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    prefix: &str,
    adapter: &dyn LanguageAdapter,
    floor: Option<&str>,
) -> bool {
    let vis_capture = format!("{prefix}.vis");
    if !query.capture_names().iter().any(|n| *n == vis_capture) {
        return true;
    }
    let Some(floor) = floor else {
        return true;
    };
    adapter.visibility_at_least_for(
        prefix,
        capture_text(query, m, source, &vis_capture).map(str::trim),
        floor,
    )
}

/// Build an item for a plain `<kind>.def` / `<kind>.name` capture pair.
/// The capture prefix is the kind string. Visibility filtering applies
/// only when the query declares `<kind>.vis` (see [`vis_gate_passes`]);
/// lexical-privacy languages filter by name via
/// [`LanguageAdapter::item_is_public_name`].
fn build_named(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    adapter: &dyn LanguageAdapter,
    prefix: &str,
    floor: Option<&str>,
) -> Option<ExtractedItem> {
    if !vis_gate_passes(query, m, source, prefix, adapter, floor) {
        return None;
    }
    let def_node = capture_node(query, m, &format!("{prefix}.def"))?;
    let name = capture_text(query, m, source, &format!("{prefix}.name"))?.to_string();
    if !adapter.item_is_public_name(&name) {
        return None;
    }
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    if let (Some(vis), Some(floor)) = (
        adapter.attribute_visibility(prefix, &attributes),
        floor.or_else(|| adapter.default_visibility_floor()),
    ) {
        if !adapter.visibility_at_least_for(prefix, Some(vis), floor) {
            return None;
        }
    }
    Some(ExtractedItem {
        kind: prefix.to_string(),
        name,
        attributes,
        span,
    })
}

fn build_impl(query: &Query, m: &QueryMatch, source: &str) -> Option<ExtractedItem> {
    let def_node = capture_node(query, m, "impl.def")?;
    let target = capture_text(query, m, source, "impl.type")?
        .trim()
        .to_string();
    let trait_text = capture_text(query, m, source, "impl.trait").map(|s| s.trim().to_string());
    let name = match trait_text {
        Some(t) if !t.is_empty() => format!("{t} for {target}"),
        _ => target,
    };
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    Some(ExtractedItem {
        kind: "impl".to_string(),
        name,
        attributes,
        span,
    })
}

/// Build an item for a member of a type (`inherent_<kind>.*` captures):
/// Rust inherent-impl associated items, Dart class/mixin/extension
/// members. The item is named `<HostBase><sep><member>` with the
/// adapter's path separator (generics + path qualifiers stripped from the
/// host so the name matches what an author writes). Visibility filtering
/// per [`vis_gate_passes`] plus the adapter's name rule.
fn build_member(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    adapter: &dyn LanguageAdapter,
    prefix: &str,
    floor: Option<&str>,
) -> Option<ExtractedItem> {
    let kind = prefix
        .strip_prefix("inherent_")
        .expect("checked starts_with");
    if !vis_gate_passes(query, m, source, prefix, adapter, floor) {
        return None;
    }
    // Members whose captured annotations mark them as inherited contract
    // (e.g. Dart's `@override`) are not new public surface — skip.
    if capture_texts(query, m, source, "inherent.annotation")
        .iter()
        .any(|a| adapter.annotation_suppresses_item(a))
    {
        return None;
    }
    let def_node = capture_node(query, m, &format!("{prefix}.def"))?;
    let host = capture_text(query, m, source, "inherent.host")?;
    let member = capture_text(query, m, source, &format!("{prefix}.name"))?;
    let name = format!(
        "{}{}{member}",
        super::sync::type_base(host),
        adapter.path_separator()
    );
    if !adapter.item_is_public_name(&name) {
        return None;
    }
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    Some(ExtractedItem {
        kind: kind.to_string(),
        name,
        attributes,
        span,
    })
}

/// A re-export is gated by the spec's own `re_export.visibility` enum,
/// not by the floor: when the query declares `re_export.vis`, the adapter
/// maps the captured modifier to a spec value or `None` (drop) — #104.
fn build_re_export(
    query: &Query,
    m: &QueryMatch,
    source: &str,
    adapter: &dyn LanguageAdapter,
) -> Option<ExtractedItem> {
    if query.capture_names().contains(&"re_export.vis") {
        let vis = capture_text(query, m, source, "re_export.vis")?.trim();
        adapter.re_export_visibility(vis)?;
    }
    let def_node = capture_node(query, m, "re_export.def")?;
    let target = capture_text(query, m, source, "re_export.target")?
        .trim()
        .to_string();
    // Extract a short name for matching purposes: the last segment of the
    // target path, after the language's separator or `as`.
    // `pub use foo::bar as Baz;` → "Baz"; `pub use foo::bar;` → "bar";
    // TypeScript `export { a as b }` → "b"; `export * from "./x"` → "./x".
    let name = re_export_name(&target, adapter.path_separator());
    let span = node_span(def_node);
    let attributes = preceding_attributes(def_node, source);
    Some(ExtractedItem {
        kind: "re_export".to_string(),
        name,
        attributes,
        span,
    })
}

fn re_export_name(target: &str, separator: &str) -> String {
    let trimmed = target.trim().trim_end_matches(';').trim();
    // A quoted module specifier (`"./barrel"`, `'./x'`) names the whole
    // module: strip the quotes and never split it — a `.` separator would
    // otherwise cut a relative path in half.
    if let Some(inner) = quoted_inner(trimmed) {
        return inner.to_string();
    }
    // Handle `path::to::thing as Alias` — after `as`.
    if let Some(idx) = trimmed.rfind(" as ") {
        return trimmed[idx + 4..].trim().to_string();
    }
    // Otherwise the last separator segment.
    trimmed
        .rsplit(separator)
        .next()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| trimmed.to_string())
}

/// The text inside a matching pair of `"` or `'` quotes, if `s` is exactly
/// one quoted string.
fn quoted_inner(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let q = bytes[0];
        if (q == b'"' || q == b'\'') && bytes[bytes.len() - 1] == q {
            return Some(&s[1..s.len() - 1]);
        }
    }
    None
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
        kind: "extern_block".to_string(),
        name,
        attributes,
        span,
    })
}

fn extern_abi(node: Node<'_>, source: &str) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_literal" {
            return Some(source[child.byte_range()].trim_matches('"').to_string());
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

/// All texts for a (possibly quantified) capture name in this match.
fn capture_texts<'a>(query: &Query, m: &QueryMatch, source: &'a str, name: &str) -> Vec<&'a str> {
    let Some(idx) = query.capture_names().iter().position(|n| *n == name) else {
        return Vec::new();
    };
    m.captures
        .iter()
        .filter(|c| c.index as usize == idx)
        .map(|c| &source[c.node.byte_range()])
        .collect()
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
        extract_items_from_source(source, "rs", &RustAdapter, None).expect("extract")
    }

    fn extract_with_floor(source: &str, floor: &str) -> Vec<ExtractedItem> {
        extract_items_from_source(source, "rs", &RustAdapter, Some(floor)).expect("extract")
    }

    fn names(items: &[ExtractedItem], kind: &str) -> Vec<String> {
        items
            .iter()
            .filter(|i| i.kind == kind)
            .map(|i| i.name.clone())
            .collect()
    }

    // #103: the eight top-level kinds that used to carry no `.vis` capture
    // extracted private items as public. Under the default floor they drop.
    #[test]
    fn private_top_level_data_items_are_not_items() {
        let source = "const A: u8 = 1;\nstatic B: u8 = 2;\nstruct S;\nenum E { X }\n\
                      union U { a: u8 }\ntrait T {}\ntype Y = u8;\n\
                      pub const PA: u8 = 1;\npub static PB: u8 = 2;\npub struct PS;\n\
                      pub enum PE { X }\npub union PU { a: u8 }\npub trait PT {}\npub type PY = u8;\n";
        let items = extract(source);
        let mut got: Vec<String> = items.iter().map(|i| i.name.clone()).collect();
        got.sort();
        assert_eq!(got, vec!["PA", "PB", "PE", "PS", "PT", "PU", "PY"]);
    }

    // `macro_rules!` takes no visibility modifier in the grammar; its
    // export is `#[macro_export]` (#254). Without it the macro is
    // crate-local: it drops under the default `pub` floor and stays under
    // a `pub(crate)` floor.
    #[test]
    fn macro_rules_is_public_only_with_macro_export() {
        let source = "#[macro_export]\nmacro_rules! exported { () => {} }\n\
                      macro_rules! local { () => {} }\n";
        assert_eq!(names(&extract(source), "macro"), vec!["exported"]);
        assert_eq!(
            names(&extract_with_floor(source, "pub(crate)"), "macro"),
            vec!["exported", "local"]
        );
    }

    // #101(b): a `pub(crate)` floor admits `pub` and `pub(crate)`, still
    // drops `pub(super)` and private, for free fns, data items and
    // inherent members alike.
    #[test]
    fn pub_crate_floor_admits_crate_wide_surface() {
        let source = "pub fn a() {}\npub(crate) fn b() {}\npub(super) fn c() {}\nfn d() {}\n\
                      pub(crate) const K: u8 = 1;\nconst P: u8 = 2;\n\
                      pub struct H;\nimpl H { pub fn m1(&self) {} pub(crate) fn m2(&self) {} fn m3(&self) {} }\n";
        let items = extract_with_floor(source, "pub(crate)");
        assert_eq!(names(&items, "fn"), vec!["a", "b", "H::m1", "H::m2"]);
        assert_eq!(names(&items, "const"), vec!["K"]);
        // The same source under the default (`pub`) floor keeps only `pub`.
        let items = extract(source);
        assert_eq!(names(&items, "fn"), vec!["a", "H::m1"]);
        assert!(names(&items, "const").is_empty());
    }

    // #104: `re_export` follows the spec enum (`pub` | `crate`), not the
    // floor — `pub(crate) use` stays under a `pub` floor, `pub(super) use`
    // drops under any floor, bare `use` never matches.
    #[test]
    fn re_export_admits_what_the_spec_enum_admits() {
        let source = "pub use a::B;\npub(crate) use a::C;\npub(super) use a::D;\nuse a::E;\n";
        assert_eq!(names(&extract(source), "re_export"), vec!["B", "C"]);
        assert_eq!(
            names(&extract_with_floor(source, "pub(crate)"), "re_export"),
            vec!["B", "C"]
        );
    }

    #[test]
    fn extracts_top_level_fn() {
        let source = "pub fn foo(x: u32) -> u32 { x + 1 }\n";
        let items = extract(source);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, "fn");
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
            .filter(|i| i.kind == "fn")
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
            !items.iter().any(|i| i.kind == "fn"),
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
        assert_eq!(items[0].kind, "fn");
        assert_eq!(items[0].name, "it_works");
        assert_eq!(items[0].attributes, vec!["#[test]"]);
    }

    #[test]
    fn extracts_struct_enum_const() {
        let source = "pub struct S;\npub enum E { A, B }\npub const N: u32 = 4;\n";
        let items = extract(source);
        assert_eq!(items.len(), 3);
        let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        assert!(kinds.contains(&"struct"));
        assert!(kinds.contains(&"enum"));
        assert!(kinds.contains(&"const"));
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
            !items.iter().any(|i| i.kind == "impl" && i.name == "T"),
            "inherent impl block must not be an item"
        );
        assert!(
            items.iter().any(|i| i.kind == "fn" && i.name == "T::one"),
            "public inherent method must extract as `T::one`"
        );
        assert!(
            !items.iter().any(|i| i.name == "T::helper"),
            "private inherent method must not be an item"
        );
        // The trait impl stays exactly one item; its method is not surfaced.
        let impls: Vec<_> = items.iter().filter(|i| i.kind == "impl").collect();
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
            items
                .iter()
                .any(|i| i.kind == "fn" && i.name == "RateLimiter::try_consume"),
            "expected `RateLimiter::try_consume`, got {:?}",
            items.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
    }

    // Public associated const / type in an inherent impl are items.
    #[test]
    fn inherent_assoc_const_and_type() {
        let source = "impl Widget { pub const MAX: u32 = 8; pub type Out = u16; fn p() {} }\n";
        let items = extract(source);
        assert!(items
            .iter()
            .any(|i| i.kind == "const" && i.name == "Widget::MAX"));
        assert!(items
            .iter()
            .any(|i| i.kind == "type" && i.name == "Widget::Out"));
    }

    #[test]
    fn re_export_name_from_alias() {
        assert_eq!(re_export_name("foo::bar as Baz", "::"), "Baz");
        assert_eq!(re_export_name("foo::bar", "::"), "bar");
        assert_eq!(re_export_name("crate::Inner;", "::"), "Inner");
    }

    #[test]
    fn extracts_re_export_with_alias() {
        let source = "pub use crate::inner::Inner as PublicName;\n";
        let items = extract(source);
        let re_exports: Vec<_> = items.iter().filter(|i| i.kind == "re_export").collect();
        assert_eq!(re_exports.len(), 1);
        assert_eq!(re_exports[0].name, "PublicName");
    }

    #[test]
    fn extracts_static_and_type() {
        let source = "pub static N: u32 = 0;\npub type Alias = u32;\n";
        let items = extract(source);
        let kinds: Vec<&str> = items.iter().map(|i| i.kind.as_str()).collect();
        assert!(kinds.contains(&"static"));
        assert!(kinds.contains(&"type"));
    }

    #[test]
    fn extracts_extern_block() {
        let source = "extern \"C\" {\n    fn foo() -> u32;\n}\n";
        let items = extract(source);
        let externs: Vec<_> = items.iter().filter(|i| i.kind == "extern_block").collect();
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
        let fns: Vec<_> = items.iter().filter(|i| i.kind == "fn").collect();
        assert!(fns.iter().any(|i| i.name == "outer"));
        assert!(
            !fns.iter().any(|i| i.name == "inner"),
            "function-body locals must not be items"
        );
    }
}
