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

impl LanguageAdapter for TypeScriptAdapter {
    fn manifest_facts(
        &self,
        project_root: &std::path::Path,
    ) -> Option<crate::manifest_facts::ManifestFacts> {
        crate::manifest_facts::npm(project_root)
    }

    fn is_inert_top_level(&self, kind: &str) -> bool {
        kind.contains("comment") || ["import_statement"].contains(&kind)
    }

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
        [".test.ts", ".test.tsx", ".spec.ts", ".spec.tsx"]
            .iter()
            .any(|s| name.ends_with(s))
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
    /// A relative specifier from the importing file's directory, or a
    /// `tsconfig` `paths` alias; each tried as written, with `.ts` / `.tsx`,
    /// and as a directory `index`. A package specifier is external.
    fn resolve_import(
        &self,
        spec: &str,
        from: &Path,
        ctx: &super::ImportContext,
    ) -> Vec<std::path::PathBuf> {
        let s = spec.trim();
        let mut bases: Vec<std::path::PathBuf> = Vec::new();
        if s.starts_with("./") || s.starts_with("../") {
            let dir = from.parent().unwrap_or(Path::new(""));
            bases.push(super::normalize_rel(&dir.join(s)));
        } else {
            for (pattern, targets) in &ctx.aliases {
                let rest = match pattern.strip_suffix('*') {
                    Some(prefix) => s.strip_prefix(prefix),
                    None => (s == pattern).then_some(""),
                };
                let Some(rest) = rest else { continue };
                for target in targets {
                    bases.push(super::normalize_rel(Path::new(
                        &target.replacen('*', rest, 1),
                    )));
                }
            }
        }
        let mut out = Vec::new();
        for b in bases {
            out.push(b.clone());
            for ext in ["ts", "tsx"] {
                out.push(b.with_extension(ext));
            }
            for index in ["index.ts", "index.tsx"] {
                out.push(b.join(index));
            }
        }
        out
    }

    /// `compilerOptions.paths` from `tsconfig.json`, made project-relative
    /// through `compilerOptions.baseUrl` (default the project root).
    fn import_aliases(&self, project_root: &Path) -> Vec<(String, Vec<String>)> {
        let Ok(text) = std::fs::read_to_string(project_root.join("tsconfig.json")) else {
            return Vec::new();
        };
        // tsconfig allows `//` comments; drop whole-line ones before parsing.
        let text: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Vec::new();
        };
        let opts = &v["compilerOptions"];
        let base = opts["baseUrl"].as_str().unwrap_or(".");
        let Some(paths) = opts["paths"].as_object() else {
            return Vec::new();
        };
        paths
            .iter()
            .map(|(pattern, targets)| {
                let targets = targets
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_str())
                    .map(|t| {
                        super::normalize_rel(&Path::new(base).join(t))
                            .to_string_lossy()
                            .into_owned()
                    })
                    .collect();
                (pattern.clone(), targets)
            })
            .collect()
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
