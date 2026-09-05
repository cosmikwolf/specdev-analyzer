//! Language adapters for the audit pipeline.
//!
//! Each adapter binds a spec `language` string to a tree-sitter grammar,
//! an embedded `.scm` query, and a per-language parameter parser. The
//! rest of the analyzer must never switch on language by name — it
//! resolves an adapter through [`adapter_for`] and calls trait methods.

pub mod c;
pub mod cpp;
pub mod dart;
pub mod rust;
pub mod typescript;

use tree_sitter::{Language, Node};

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
pub fn adapter_for(language: &str) -> Option<Box<dyn LanguageAdapter>> {
    match language {
        "rust" => Some(Box::new(rust::RustAdapter)),
        "c" => Some(Box::new(c::CAdapter)),
        "cpp" | "c++" => Some(Box::new(cpp::CppAdapter)),
        "dart" => Some(Box::new(dart::DartAdapter)),
        "typescript" => Some(Box::new(typescript::TypeScriptAdapter)),
        _ => None,
    }
}
