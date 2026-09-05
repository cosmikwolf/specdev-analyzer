//! TypeScript adapter — tree-sitter-typescript + embedded `audit.scm`.
//!
//! One `[project].language` spans two extensions and two grammars: `.ts`
//! parses with `LANGUAGE_TYPESCRIPT`, `.tsx` with `LANGUAGE_TSX`. The TSX
//! grammar cannot parse `<T>x` type assertions and the TypeScript grammar
//! cannot parse JSX, so the choice is per file, keyed off the resolved
//! path's extension. The query text is identical under both grammars
//! (node names match); the `Query` object compiles per grammar.
//!
//! Visibility is `export`: a top-level declaration is public when wrapped
//! in an `export_statement`, and a class member is public unless it carries
//! `private` / `protected` or a `#`-prefixed name. The `#` rule is
//! lexical and lives in `item_is_public_name`; the modifier rules live in
//! `visibility_at_least_for`, which reads the capture prefix to tell a
//! top-level `.vis` (absent = private) from a member `.vis` (absent =
//! public). The floor string is `export`; it is the only floor the
//! language has, so a profile has nothing to declare.

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct TypeScriptAdapter;

const QUERY: &str = include_str!("../../../specdev/languages/typescript/audit.scm");
const ITEMS_QUERY: &str = include_str!("../../../specdev/languages/typescript/items.scm");

impl LanguageAdapter for TypeScriptAdapter {
    fn name(&self) -> &'static str {
        "typescript"
    }

    fn file_extension(&self) -> &'static str {
        "ts"
    }

    fn file_extensions(&self) -> Vec<&'static str> {
        vec!["ts", "tsx"]
    }

    fn path_separator(&self) -> &'static str {
        "."
    }

    fn tree_sitter_language(&self) -> Language {
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
    }

    fn tree_sitter_language_for(&self, ext: &str) -> Language {
        if ext == "tsx" {
            tree_sitter_typescript::LANGUAGE_TSX.into()
        } else {
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()
        }
    }

    fn embedded_query(&self) -> &'static str {
        QUERY
    }

    fn items_query(&self) -> Option<&'static str> {
        Some(ITEMS_QUERY)
    }

    fn default_visibility_floor(&self) -> Option<&'static str> {
        // The one floor the language has: an item must be exported.
        Some("export")
    }

    fn visibility_at_least(&self, captured: Option<&str>, floor: &str) -> bool {
        // Top-level rule: the captured `.vis` is the `export` keyword or
        // nothing. Nothing written means module-private.
        captured.map(str::trim) == Some(floor)
    }

    fn visibility_at_least_for(&self, prefix: &str, captured: Option<&str>, floor: &str) -> bool {
        if prefix.starts_with("inherent_") {
            // Member rule: absent accessibility modifier (or `public`) is
            // public; `private` / `protected` are not contract surface.
            // The host being exported is the query's job (the member
            // patterns require an enclosing `export_statement`).
            !matches!(captured.map(str::trim), Some("private") | Some("protected"))
        } else {
            self.visibility_at_least(captured, floor)
        }
    }

    fn module_file_candidates(&self, snake: &str) -> Vec<String> {
        // `<snake>/index.ts` is the ESM directory-module convention — the
        // analog of Rust's `<snake>/mod.rs`. Components are `.tsx`.
        vec![
            format!("{snake}.ts"),
            format!("{snake}.tsx"),
            format!("{snake}/index.ts"),
            format!("{snake}/index.tsx"),
        ]
    }

    fn lockfile_candidates(&self) -> &'static [&'static str] {
        // A lockfile is a package-manager fact; the language has three.
        &["pnpm-lock.yaml", "package-lock.json", "yarn.lock"]
    }

    fn item_is_public_name(&self, name: &str) -> bool {
        // ECMAScript `#private` members are private by name: `Store.#cache`
        // is never contract surface, whatever modifiers say.
        name.split('.').all(|seg| !seg.starts_with('#'))
    }

    fn parameter_node_kind(&self) -> &'static str {
        "required_parameter"
    }

    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter> {
        // `required_parameter` / `optional_parameter`: the name sits behind
        // the `pattern` field (an `identifier` for the plain case; a
        // destructuring pattern otherwise, which we skip), the type behind
        // `type` as a `type_annotation` whose text starts with `: `.
        let pattern = node.child_by_field_name("pattern")?;
        if pattern.kind() != "identifier" {
            return None;
        }
        let name = source[pattern.byte_range()].to_string();
        let type_name = node
            .child_by_field_name("type")
            .map(|t| {
                source[t.byte_range()]
                    .trim_start_matches(':')
                    .trim()
                    .to_string()
            })
            .unwrap_or_default();
        Some(ParsedParameter { name, type_name })
    }
}
