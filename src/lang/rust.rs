//! Rust adapter — tree-sitter-rust + embedded `audit.scm`.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct RustAdapter;

const QUERY: &str = include_str!("../embedded/languages/rust/audit.scm");
const ITEMS_QUERY: &str = include_str!("../embedded/languages/rust/items.scm");
const TESTS_QUERY: &str = include_str!("../embedded/languages/rust/tests.scm");
const IMPORTS_QUERY: &str = include_str!("../embedded/languages/rust/imports.scm");

impl LanguageAdapter for RustAdapter {
    fn manifest_facts(&self, project_root: &std::path::Path) -> Option<crate::manifest_facts::ManifestFacts> {
        crate::manifest_facts::cargo(project_root)
    }

    fn is_inert_top_level(&self, kind: &str) -> bool {
        kind.contains("comment") || ["attribute_item", "inner_attribute_item", "use_declaration", "extern_crate_declaration"].contains(&kind)
    }

    fn tests_query(&self) -> Option<&'static str> {
        Some(TESTS_QUERY)
    }

    fn conventional_test_roots(&self) -> &'static [&'static str] {
        &["tests"]
    }

    fn imports_query(&self) -> Option<&'static str> {
        Some(IMPORTS_QUERY)
    }

    /// `crate::a::b`, `super::…`, `self::…`, and from a crate-root file a
    /// bare `a::b` naming a sibling module; brace lists and `as` aliases
    /// expanded. Any other root (another crate, `std`, `core`) is external.
    fn resolve_import(&self, spec: &str, from: &std::path::Path, ctx: &super::ImportContext) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for path in expand_use(spec) {
            let mut segs: Vec<&str> = path.split("::").map(str::trim).filter(|s| !s.is_empty()).collect();
            while matches!(segs.last(), Some(&"self") | Some(&"*")) {
                segs.pop();
            }
            let Some(first) = segs.first().copied() else { continue };
            let (base, rest): (std::path::PathBuf, &[&str]) = match first {
                "crate" => (ctx.source_root.clone(), &segs[1..]),
                "self" => (module_dir(from), &segs[1..]),
                "super" => {
                    let mut dir = module_dir(from);
                    let mut i = 0;
                    while segs.get(i) == Some(&"super") {
                        dir = dir.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
                        i += 1;
                    }
                    (dir, &segs[i..])
                }
                _ if is_crate_root(from, &ctx.source_root) => (ctx.source_root.clone(), &segs[..]),
                _ => continue,
            };
            // The longest prefix that is a module file wins.
            for k in (1..=rest.len()).rev() {
                let mut dir = base.clone();
                for s in &rest[..k - 1] {
                    dir.push(s);
                }
                out.push(dir.join(format!("{}.rs", rest[k - 1])));
                out.push(dir.join(rest[k - 1]).join("mod.rs"));
            }
        }
        out
    }

    fn name(&self) -> &'static str {
        "rust"
    }

    fn file_extension(&self) -> &'static str {
        "rs"
    }

    fn tree_sitter_language(&self) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn embedded_query(&self) -> &'static str {
        QUERY
    }

    fn items_query(&self) -> Option<&'static str> {
        Some(ITEMS_QUERY)
    }

    fn module_file_candidates(&self, snake: &str) -> Vec<String> {
        vec![format!("{snake}.rs"), format!("{snake}/mod.rs")]
    }

    fn lockfile_candidates(&self) -> &'static [&'static str] {
        &["Cargo.lock"]
    }

    fn toolchain_file_candidates(&self) -> &'static [&'static str] {
        &["rust-toolchain.toml", "rust-toolchain"]
    }

    fn default_visibility_floor(&self) -> Option<&'static str> {
        Some("pub")
    }

    fn visibility_at_least(&self, captured: Option<&str>, floor: &str) -> bool {
        visibility_rank(captured) >= visibility_rank(Some(floor))
    }

    fn re_export_visibility(&self, captured: &str) -> Option<&'static str> {
        match normalize_visibility(captured).as_str() {
            "pub" => Some("pub"),
            "pub(crate)" => Some("crate"),
            _ => None,
        }
    }

    fn item_identity_key(&self, kind: &str, name: &str) -> String {
        // Trait-impl names carry generics and path qualifiers the author
        // shouldn't have to track; normalize them out of the join key.
        if kind == "impl" {
            crate::items::sync::normalize_impl_name(name)
        } else {
            name.to_string()
        }
    }

    fn is_test_function(&self, item: &crate::items::ExtractedItem) -> bool {
        item.kind == "fn" && item.attributes.iter().any(|a| a.contains("test"))
    }

    fn parameter_node_kind(&self) -> &'static str {
        "parameter"
    }

    fn self_node_kind(&self) -> Option<&'static str> {
        Some("self_parameter")
    }

    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        let mut name = String::new();
        let mut type_name = String::new();
        let mut cursor = node.walk();

        for child in node.children(&mut cursor) {
            match child.kind() {
                "identifier" if name.is_empty() => {
                    name = source[child.byte_range()].to_string();
                }
                "reference_type"
                | "type_identifier"
                | "generic_type"
                | "scoped_type_identifier"
                | "primitive_type"
                | "array_type"
                | "tuple_type" => {
                    type_name = source[child.byte_range()].to_string();
                }
                _ => {}
            }
        }

        if name.is_empty() {
            None
        } else {
            Some(ParsedParameter { name, type_name })
        }
    }

    fn parse_self_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        let text = source[node.byte_range()].to_string();
        Some(ParsedParameter {
            name: "self".to_string(),
            type_name: text,
        })
    }
}

/// Collapse whitespace inside a visibility modifier so `pub (crate)` and
/// `pub(crate)` compare equal.
fn normalize_visibility(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Rust's visibility lattice as a rank. `pub` is the widest; `pub(self)`
/// and no modifier are private. `pub(in path)` is treated like
/// `pub(super)`: narrower than the crate, wider than the module.
fn visibility_rank(vis: Option<&str>) -> u8 {
    match vis.map(normalize_visibility).as_deref() {
        Some("pub") => 3,
        Some("pub(crate)") => 2,
        Some("pub(super)") => 1,
        Some(v) if v.starts_with("pub(in") => 1,
        _ => 0,
    }
}


/// The directory a file's child modules live in: its own directory for a
/// `mod.rs` / `lib.rs` / `main.rs`, else `<dir>/<stem>/`.
fn module_dir(file: &std::path::Path) -> std::path::PathBuf {
    let parent = file.parent().unwrap_or(std::path::Path::new("")).to_path_buf();
    match file.file_name().and_then(|n| n.to_str()) {
        Some("mod.rs") | Some("lib.rs") | Some("main.rs") => parent,
        _ => parent.join(file.file_stem().unwrap_or_default()),
    }
}

fn is_crate_root(file: &std::path::Path, source_root: &std::path::Path) -> bool {
    file.parent() == Some(source_root)
        && matches!(file.file_name().and_then(|n| n.to_str()), Some("lib.rs") | Some("main.rs"))
}

/// `a::{b, c::{d, e}} as x` → `a::b`, `a::c::d`, `a::c::e`.
fn expand_use(spec: &str) -> Vec<String> {
    let spec = spec.trim().trim_end_matches(';');
    let Some(open) = spec.find('{') else {
        return vec![spec.split(" as ").next().unwrap_or(spec).trim().to_string()];
    };
    let prefix = spec[..open].trim_end_matches("::").trim();
    let Some(close) = spec.rfind('}') else {
        return vec![prefix.to_string()];
    };
    let inner = &spec[open + 1..close];
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&inner[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .flat_map(|p| {
            expand_use(p)
                .into_iter()
                .map(|s| if prefix.is_empty() { s } else { format!("{prefix}::{s}") })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod visibility_tests {
    use super::*;
    use crate::lang::LanguageAdapter;

    #[test]
    fn pub_floor_admits_only_pub() {
        let a = RustAdapter;
        assert!(a.visibility_at_least(Some("pub"), "pub"));
        assert!(!a.visibility_at_least(Some("pub(crate)"), "pub"));
        assert!(!a.visibility_at_least(Some("pub(super)"), "pub"));
        assert!(!a.visibility_at_least(None, "pub"));
    }

    #[test]
    fn crate_floor_admits_pub_and_pub_crate() {
        let a = RustAdapter;
        assert!(a.visibility_at_least(Some("pub"), "pub(crate)"));
        assert!(a.visibility_at_least(Some("pub (crate)"), "pub(crate)"));
        assert!(!a.visibility_at_least(Some("pub(super)"), "pub(crate)"));
        assert!(!a.visibility_at_least(Some("pub(in crate::x)"), "pub(crate)"));
        assert!(!a.visibility_at_least(None, "pub(crate)"));
    }

    #[test]
    fn re_export_visibility_follows_the_spec_enum() {
        let a = RustAdapter;
        assert_eq!(a.re_export_visibility("pub"), Some("pub"));
        assert_eq!(a.re_export_visibility("pub(crate)"), Some("crate"));
        assert_eq!(a.re_export_visibility("pub(super)"), None);
        assert_eq!(a.re_export_visibility("pub(in crate::a)"), None);
    }
}
