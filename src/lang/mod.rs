//! Language adapters for the audit pipeline.
//!
//! Each adapter binds a spec `language` string to a tree-sitter grammar,
//! an embedded `.scm` query, and a per-language parameter parser. The
//! rest of the analyzer must never switch on language by name — it
//! resolves an adapter through [`adapter_for`] and calls trait methods.

pub mod c;
pub mod cpp;
pub mod dart;
pub mod python;
pub mod rust;
pub mod typescript;

use std::path::{Component, Path, PathBuf};

use tree_sitter::{Language, Node};

/// Relative path from directory `from_dir` to file `to_file` (both
/// project-root-relative, `Normal` components only), joined with `/`.
/// `./`-prefixed when the result does not start with `..`, so the string
/// is always a relative import and never a bare package name.
pub fn relative_import_path(from_dir: &Path, to_file: &Path) -> String {
    let normal = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    };
    let from = normal(from_dir);
    let to = normal(to_file);
    let common = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = Vec::new();
    for _ in common..from.len() {
        parts.push("..".to_string());
    }
    parts.extend(to[common..].iter().cloned());
    let joined = parts.join("/");
    if joined.starts_with("..") {
        joined
    } else {
        format!("./{joined}")
    }
}

/// A parsed function parameter, as observed in source code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedParameter {
    pub name: String,
    pub type_name: String,
}

/// A tree-sitter-backed language adapter.
pub trait LanguageAdapter: Send + Sync {
    /// Canonical name — must match `spec.toml`'s `project.language`.
    fn name(&self) -> &'static str;

    /// Primary source file extension — the one `<snake>.<ext>` module
    /// resolution tries first and the one authoring prose names.
    fn file_extension(&self) -> &'static str;

    /// Every source extension this language spans. Default: the primary
    /// one. A language with several (TypeScript: `ts` + `tsx`) lists them
    /// all; walks over reference sources and candidate paths take the
    /// list, never the primary alone.
    fn file_extensions(&self) -> Vec<&'static str> {
        vec![self.file_extension()]
    }

    /// Tree-sitter grammar for a source file with extension `ext`. A
    /// language may span several grammars (TypeScript ships one for `.ts`
    /// and one for `.tsx`; the TSX grammar cannot parse `<T>x` type
    /// assertions, so the choice is real). Default: the single grammar
    /// from [`tree_sitter_language`](Self::tree_sitter_language),
    /// whatever the extension. Extraction keys this off the *resolved*
    /// path, because `source_file` makes the extension per-module data.
    fn tree_sitter_language_for(&self, _ext: &str) -> Language {
        self.tree_sitter_language()
    }

    /// Segment separator for qualified paths in this language
    /// (`::` for Rust/C++, `.` for Dart). Anything that splits or joins
    /// qualified identifiers must take the separator from here rather
    /// than assuming `::`.
    fn path_separator(&self) -> &'static str {
        "::"
    }

    /// Tree-sitter grammar for this language.
    fn tree_sitter_language(&self) -> Language;

    /// The built-in `.scm` query source. Adapters embed this via `include_str!`
    /// so the analyzer binary is self-contained; plugins can still override
    /// by supplying their own query via `queries_dir`.
    fn embedded_query(&self) -> &'static str;

    /// The built-in `items.scm` query source for `extract-items`, if the
    /// language tier ships one. The item-kind vocabulary itself is data —
    /// loaded at runtime from the project's inlined
    /// `languages/<lang>/items.schema.yaml` — and the query's capture
    /// prefixes must use the same kind strings (`<kind>.def` /
    /// `<kind>.name`, plus `inherent_<kind>.*` + `inherent.host` for
    /// qualified members).
    fn items_query(&self) -> Option<&'static str> {
        None
    }

    /// The built-in `tests.scm` query source for test discovery, if the
    /// language tier ships one. Its captures are `@test`, `@test.name`,
    /// `@test.suite`, `@test.unnamed`, `@test.pending`, `@group` and
    /// `@group.name` (see [`crate::test_discovery`]); framework names stay in
    /// the query, never in the analyzer.
    fn tests_query(&self) -> Option<&'static str> {
        None
    }

    /// How a test whose bare name is not unique is named.
    fn test_identity_style(&self) -> TestIdentityStyle {
        TestIdentityStyle::ModuleOrFile
    }

    /// Project-relative directories this language keeps tests in by
    /// convention, walked in addition to `[paths].source` and
    /// `[paths].tests`.
    fn conventional_test_roots(&self) -> &'static [&'static str] {
        &[]
    }

    /// Whether a file (project-relative, one of this language's extensions)
    /// holds tests by this language's convention.
    fn is_test_file(&self, _rel: &Path) -> bool {
        true
    }

    /// Whether a top-level syntax node of this kind is not code of its own
    /// (a comment, an attribute, an import). A source file holds only tests
    /// when every other top-level node sits inside a test or a test group.
    fn is_inert_top_level(&self, kind: &str) -> bool {
        kind.contains("comment")
    }

    /// The built-in `imports.scm` query source, if the language tier ships
    /// one: `@import.source` for each import specifier and `@import.reexport`
    /// for each re-export (a barrel is followed through it). `check` resolves
    /// them with [`LanguageAdapter::resolve_import`] to find the internal
    /// edges between a package's own modules.
    fn imports_query(&self) -> Option<&'static str> {
        None
    }

    /// Project-relative files one import specifier (as captured, quotes
    /// stripped) may name, most specific first. The first that exists is the
    /// import's target. Empty for a specifier outside the package (another
    /// package, the standard library): external, the manifest's business.
    fn resolve_import(&self, _spec: &str, _from: &Path, _ctx: &ImportContext) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Specifier prefix aliases a project declares (TypeScript `tsconfig`
    /// `paths`): `(pattern, targets)`, both project-relative, `*` the
    /// wildcard. Read once per run.
    /// The project's own manifest (`Cargo.toml`, `package.json`, …), read
    /// for its dependencies and package fields. `None` when the language has
    /// none or the project has not created it.
    fn manifest_facts(&self, _project_root: &Path) -> Option<crate::manifest_facts::ManifestFacts> {
        None
    }

    fn import_aliases(&self, _project_root: &Path) -> Vec<(String, Vec<String>)> {
        Vec::new()
    }

    /// Files that belong to the same module as its source file (a C header
    /// twin of a `.c` file), project-relative.
    fn module_twin_files(&self, _source: &Path) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Comparable identity for joining a spec item against an extracted
    /// source item. Default: the name verbatim. Languages whose
    /// synthesized names carry purely-syntactic variance an author
    /// shouldn't have to track (Rust trait-impl generics and path
    /// qualifiers) override this to normalize the join key without
    /// mutating the displayed name.
    fn item_identity_key(&self, _kind: &str, name: &str) -> String {
        name.to_string()
    }

    /// Ordered candidate file names (relative to the source root) for a
    /// spec module whose snake_case name is `snake`. The first that exists
    /// wins. Default: `<snake>.<ext>`. Rust adds `<snake>/mod.rs`; an ESM
    /// language may add `<snake>/index.<ext>`. Callers never switch on
    /// the language name to add a layout (#101(a)).
    fn module_file_candidates(&self, snake: &str) -> Vec<String> {
        vec![format!("{snake}.{}", self.file_extension())]
    }

    /// The import specifier a file in `from_dir` uses to import `to_file`
    /// — both project-root-relative. Used by `derive` for the
    /// `{source_relative}` token in a derived file's body. Default: the
    /// relative **file path** with its extension, forward slashes, `./`
    /// prefixed when it does not start with `..` (Dart imports keep
    /// `.dart`). A language whose import grammar is not a relative path
    /// (Rust `use`) must override before a profile of that language
    /// declares `derived_files` rules. TypeScript overrides to drop the
    /// extension and the `/index` directory-module suffix.
    /// The module a name imported from `module` would be, when that name is
    /// a submodule (`from . import stream` → `.stream`). The import pass
    /// keeps it only when it resolves to a project file. Default: `None`
    /// (the language has no `@import.member` capture).
    fn member_import_spec(&self, _module: &str, _member: &str) -> Option<String> {
        None
    }

    fn import_specifier(&self, from_dir: &Path, to_file: &Path) -> String {
        relative_import_path(from_dir, to_file)
    }

    /// Lockfile names this language's package managers write, relative
    /// to the directory that owns the manifest. A lockfile is a
    /// package-manager fact, not a language fact — a language with
    /// several managers lists every name — so callers hash *every*
    /// candidate they find rather than picking one. Feeds the compile-gate
    /// key in `gen-lineage.toml`: a dependency change must invalidate a
    /// recorded gate pass. Default: none (the language has no lockfile).
    fn lockfile_candidates(&self) -> &'static [&'static str] {
        &[]
    }

    /// Toolchain-pin files that make the build environment a *project*
    /// fact (Rust: `rust-toolchain.toml`). Hashed into the gate key by the
    /// same mechanism as the lockfile, so a toolchain pin change
    /// invalidates a recorded pass without any `--version` subprocess
    /// output entering committed state. Default: none.
    fn toolchain_file_candidates(&self) -> &'static [&'static str] {
        &[]
    }

    /// The visibility floor this language applies when the profile
    /// declares none (`items.visibility_floor` absent). `None` means the
    /// language has no visibility modifiers — every declared `.vis`
    /// capture is then ignored and only the name rule filters. Rust
    /// returns `pub`. Default: `None`.
    fn default_visibility_floor(&self) -> Option<&'static str> {
        None
    }

    /// Whether a captured visibility modifier (`None` = no modifier
    /// written) is at or above `floor`. The floor is an opaque profile
    /// string; the lattice (`pub` ⊃ `pub(crate)` ⊃ `pub(super)`) is
    /// language knowledge and lives here, never in the extractor.
    /// Default: exact match only.
    fn visibility_at_least(&self, captured: Option<&str>, floor: &str) -> bool {
        captured.map(str::trim) == Some(floor)
    }

    /// [`visibility_at_least`](Self::visibility_at_least) with the capture
    /// prefix (`fn`, `inherent_fn`, `inherent_function`, …) in hand, for
    /// languages whose "no modifier written" means different things at
    /// different positions: in TypeScript an absent `export` on a
    /// top-level declaration is private, while an absent accessibility
    /// modifier on a class member is public. Rust's lattice is uniform,
    /// so the default ignores the prefix and delegates. The extractor
    /// calls this one; `visibility_at_least` remains the lattice itself.
    fn visibility_at_least_for(&self, _prefix: &str, captured: Option<&str>, floor: &str) -> bool {
        self.visibility_at_least(captured, floor)
    }

    /// The visibility an item takes from its attributes, for a kind whose
    /// grammar node carries no visibility modifier. The extractor gates it
    /// against the floor like a captured modifier. Default: `None` (no
    /// such kind; the item is not gated).
    fn attribute_visibility(&self, _prefix: &str, _attributes: &[String]) -> Option<&'static str> {
        None
    }

    /// Map a re-export's captured visibility modifier to the spec-side
    /// `re_export.visibility` enum value, or `None` when the spec cannot
    /// express it (the item then drops). Rust: `pub` → `pub`,
    /// `pub(crate)` → `crate`, anything else → `None` (#104). Default:
    /// `None` — a language that declares no `re_export` kind never gets
    /// here.
    fn re_export_visibility(&self, _captured: &str) -> Option<&'static str> {
        None
    }

    /// Whether an extracted item name is public contract surface under
    /// this language's naming rules. Languages with lexical privacy
    /// (Dart's `_` prefix) override this; languages with visibility
    /// modifiers express the filter through `.vis` captures in their
    /// items query instead.
    fn item_is_public_name(&self, _name: &str) -> bool {
        true
    }

    /// Whether an extracted item is a test function for bare-name
    /// `verified_by` resolution. Default: no (languages whose tests are
    /// not attribute-marked in module source cannot resolve bare names —
    /// use `Module::name`-style qualified entries there).
    fn is_test_function(&self, _item: &crate::items::ExtractedItem) -> bool {
        false
    }

    /// Whether a member carrying this annotation name (captured as
    /// `inherent.annotation` by the language's items query) is excluded
    /// from extraction. Dart's `@override` is the motivating case: an
    /// override implements an inherited contract — the contract lives on
    /// the declaring base, so an override is never new public surface.
    /// Default: nothing suppresses.
    fn annotation_suppresses_item(&self, _annotation: &str) -> bool {
        false
    }

    /// Parse a single parameter node from this language's grammar, if the
    /// adapter recognizes its kind.
    fn parse_parameter(&self, node: Node<'_>, source: &str) -> Option<ParsedParameter>;

    /// Some languages (Rust) expose explicit `self` parameters as a distinct
    /// node kind. Override to lift them into the parameter list.
    fn parse_self_parameter(&self, _node: Node<'_>, _source: &str) -> Option<ParsedParameter> {
        None
    }

    /// The grammar's node kind for items inside a parameter list that
    /// correspond to ordinary parameters. `parse_parameter` is only called
    /// on children whose kind matches this or [`self_node_kind`].
    fn parameter_node_kind(&self) -> &'static str;

    /// Optional — the node kind for a self/receiver parameter, if any.
    fn self_node_kind(&self) -> Option<&'static str> {
        None
    }
}

/// Resolve a language name to a boxed adapter. Returns `None` for unknown
/// languages; callers surface this with their own error context.
/// What [`LanguageAdapter::resolve_import`] knows about the project.
#[derive(Debug, Clone)]
pub struct ImportContext {
    /// The code root; every path below is relative to it.
    pub code_root: PathBuf,
    /// `[paths].source`, relative to the code root.
    pub source_root: PathBuf,
    /// The project's own package name (`[project].name`).
    pub package_name: String,
    /// From [`LanguageAdapter::import_aliases`].
    pub aliases: Vec<(String, Vec<String>)>,
}

/// `p` with `.` and `..` components folded away, lexically.
pub fn normalize_rel(p: &Path) -> PathBuf {
    let mut out: Vec<std::ffi::OsString> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(s) => out.push(s.to_os_string()),
            _ => {}
        }
    }
    out.iter().collect()
}

/// How a test whose bare name is not unique is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestIdentityStyle {
    /// `<Module>::<name>` when the test sits in one spec module's source
    /// file, else `<file path>::<name>`.
    ModuleOrFile,
    /// The enclosing group titles and the test title, joined by the
    /// separator (`cart > empty > adds`).
    GroupPath(&'static str),
    /// The file, the enclosing group names and the test name, joined by the
    /// separator (`tests/test_x.py::TestA::test_b`, a pytest node id).
    FileGroupPath(&'static str),
}

pub fn adapter_for(language: &str) -> Option<Box<dyn LanguageAdapter>> {
    match language {
        "rust" => Some(Box::new(rust::RustAdapter)),
        "c" => Some(Box::new(c::CAdapter)),
        "cpp" | "c++" => Some(Box::new(cpp::CppAdapter)),
        "dart" => Some(Box::new(dart::DartAdapter)),
        "typescript" => Some(Box::new(typescript::TypeScriptAdapter)),
        "python" => Some(Box::new(python::PythonAdapter)),
        _ => None,
    }
}
