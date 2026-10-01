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

use std::path::Path;

use tree_sitter::{Language, Node};

use super::{LanguageAdapter, ParsedParameter};

pub struct TypeScriptAdapter;

const QUERY: &str = include_str!("../embedded/languages/typescript/audit.scm");
const ITEMS_QUERY: &str = include_str!("../embedded/languages/typescript/items.scm");
const IMPORTS_QUERY: &str = include_str!("../embedded/languages/typescript/imports.scm");
const TESTS_QUERY: &str = include_str!("../embedded/languages/typescript/tests.scm");

/// Node's built-in modules, importable without a declared dependency
/// (with or without the `node:` prefix, which is dropped earlier).
const NODE_BUILTINS: &[&str] = &[
    "assert",
    "async_hooks",
    "buffer",
    "child_process",
    "cluster",
    "console",
    "constants",
    "crypto",
    "dgram",
    "diagnostics_channel",
    "dns",
    "domain",
    "events",
    "fs",
    "http",
    "http2",
    "https",
    "inspector",
    "module",
    "net",
    "os",
    "path",
    "perf_hooks",
    "process",
    "punycode",
    "querystring",
    "readline",
    "repl",
    "stream",
    "string_decoder",
    "sys",
    "timers",
    "tls",
    "trace_events",
    "tty",
    "url",
    "util",
    "v8",
    "vm",
    "wasi",
    "worker_threads",
    "zlib",
];

impl LanguageAdapter for TypeScriptAdapter {
    fn tests_query(&self) -> Option<&'static str> {
        Some(TESTS_QUERY)
    }

    fn test_identity_style(&self) -> super::TestIdentityStyle {
        super::TestIdentityStyle::GroupPath(" > ")
    }

    fn conventional_test_roots(&self) -> &'static [&'static str] {
        &["."]
    }

    /// `*.test.ts(x)` and `*.spec.ts(x)` (vitest, jest, Playwright).
    fn is_test_file(&self, rel: &Path) -> bool {
        let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
        [".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx"].iter().any(|s| name.ends_with(s))
    }

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

    fn imports_query(&self) -> Option<&'static str> {
        Some(IMPORTS_QUERY)
    }

    /// `pkg`, `pkg/sub` → `pkg`; `@scope/pkg/sub` → `@scope/pkg`. Relative
    /// (`./`, `../`, `/`), path-alias (`@/`, `~/`), subpath-import (`#`)
    /// and scheme (`node:`, `data:`, …) specifiers name no package, and
    /// neither does a Node built-in.
    fn import_package(&self, specifier: &str, _self_package: &str) -> Option<String> {
        let s = specifier.trim();
        if s.is_empty()
            || s.starts_with('.')
            || s.starts_with('/')
            || s.starts_with("@/")
            || s.starts_with("~/")
            || s.starts_with('#')
            || s.contains(':')
        {
            return None;
        }
        let mut parts = s.split('/');
        let first = parts.next()?;
        let name = if first.starts_with('@') {
            format!("{first}/{}", parts.next().filter(|p| !p.is_empty())?)
        } else {
            first.to_string()
        };
        (!NODE_BUILTINS.contains(&name.as_str())).then_some(name)
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

    fn import_specifier(&self, from_dir: &Path, to_file: &Path) -> String {
        // ESM resolution under `moduleResolution: bundler`: the specifier
        // carries no `.ts`/`.tsx` extension, and a `<dir>/index.ts(x)`
        // module is imported as `<dir>`.
        let mut spec = super::relative_import_path(from_dir, to_file);
        for ext in [".tsx", ".ts"] {
            if let Some(stem) = spec.strip_suffix(ext) {
                spec = stem.to_string();
                break;
            }
        }
        if spec == "./index" {
            return ".".to_string();
        }
        if let Some(dir) = spec.strip_suffix("/index") {
            spec = dir.to_string();
        }
        spec
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
