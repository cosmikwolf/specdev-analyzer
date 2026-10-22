;; Rust items query for tree-sitter.
;;
;; Captures top-level items per the items[] schema (see
;; `languages/rust/items.schema.yaml`). Each top-level construct that
;; corresponds to a `kind` in the schema gets its own pattern. The
;; analyzer's `extract-items` subcommand walks these matches and produces
;; `ExtractedItem { kind, name, attributes[], span }` per
;; capture.
;;
;; Conventions:
;; - Each item kind has at least an `<kind>.def` capture (the whole node)
;;   and a `<kind>.name` capture (the identifier). Where attributes
;;   precede the item, capture them as `<kind>.attrs`.
;; - `mod_item` and `extern_crate_declaration` are intentionally excluded
;;   per items[] design: modules are first-class spec entities; extern
;;   crate is an architecture-level concern.
;;
;; Scope (D70): most patterns are anchored under `(source_file ...)` so  ;; PLANNING-OK: query file, plugin-internal — never copied into a project
;; only direct children of the file are extracted; nested `mod` blocks and
;; function-body locals are never items. The deliberate exception is
;; inherent `impl` bodies — a method is a function, so a PUBLIC associated
;; fn/const/type is an item named `<Type>::<member>`. The inherent impl
;; block itself produces NO item. A TRAIT impl (`impl Trait for Type`)
;; stays ONE item: the conformance is the contract; its method bodies are
;; dictated by the trait, not separate items. The `trait:` field
;; (present → trait impl) vs `!trait` (absent → inherent impl) partitions
;; the two cases; visibility is captured (`*.vis`) and compared against
;; the profile's visibility floor in the extractor (private associated
;; members are not contract surface).
;;
;; Visibility (#101(b) / #103 / #104): every top-level kind whose grammar
;; node accepts a `visibility_modifier` carries an optional `@<kind>.vis`
;; capture — fn, struct, enum, union, trait, type, const, static, and the
;; three inherent members. A declared-but-absent capture means "no
;; modifier" = private, so the item drops under any floor. `macro` has NO
;; capture: `macro_rules!` takes no visibility_modifier in the grammar (its
;; export is the `#[macro_export]` attribute), so every macro extracts.
;; `re_export` is gated by the spec's own `visibility` enum (`pub` |
;; `crate`), not by the floor — see the pattern below.

;; ──────────────────────────────────────────────────────────────────
;; fn — top-level free functions. Visibility is captured (`@fn.vis`) and
;; compared against the floor in the extractor: a free fn below the floor
;; is a module-private helper, not contract surface, and must not surface
;; as `in_source_only` drift (mirrors the inherent-method rule).
(source_file
  (function_item
    (visibility_modifier)? @fn.vis
    name: (identifier) @fn.name
    parameters: (parameters) @fn.params
    return_type: (_)? @fn.return_type) @fn.def)

;; struct
(source_file
  (struct_item
    (visibility_modifier)? @struct.vis
    name: (type_identifier) @struct.name) @struct.def)

;; enum
(source_file
  (enum_item
    (visibility_modifier)? @enum.vis
    name: (type_identifier) @enum.name) @enum.def)

;; union
(source_file
  (union_item
    (visibility_modifier)? @union.vis
    name: (type_identifier) @union.name) @union.def)

;; trait
(source_file
  (trait_item
    (visibility_modifier)? @trait.vis
    name: (type_identifier) @trait.name) @trait.def)

;; impl — TRAIT impls only: one item per `impl Trait for Type` (the
;; conformance is the contract; D70). The `trait:` field must be present,  ;; PLANNING-OK: plugin-internal
;; so inherent impls do NOT match this pattern.
(source_file
  (impl_item
    trait: (_) @impl.trait
    type: (_) @impl.type) @impl.def)

;; inherent impl associated items (D70): `impl Type { pub fn m }` produces  ;; PLANNING-OK: plugin-internal
;; no impl item — each PUBLIC associated fn/const/type is an item named
;; `<Type>::<member>`. `!trait` restricts these to inherent impls;
;; `inherent.host` carries the `<Type>` for the qualified-name prefix.
;; Visibility is captured (`*.vis`) and compared against the floor in the
;; extractor.
(source_file
  (impl_item
    !trait
    type: (_) @inherent.host
    body: (declaration_list
      (function_item
        (visibility_modifier)? @inherent_fn.vis
        name: (identifier) @inherent_fn.name) @inherent_fn.def)))

(source_file
  (impl_item
    !trait
    type: (_) @inherent.host
    body: (declaration_list
      (const_item
        (visibility_modifier)? @inherent_const.vis
        name: (identifier) @inherent_const.name) @inherent_const.def)))

(source_file
  (impl_item
    !trait
    type: (_) @inherent.host
    body: (declaration_list
      (type_item
        (visibility_modifier)? @inherent_type.vis
        name: (type_identifier) @inherent_type.name) @inherent_type.def)))

;; type alias
(source_file
  (type_item
    (visibility_modifier)? @type.vis
    name: (type_identifier) @type.name) @type.def)

;; const
(source_file
  (const_item
    (visibility_modifier)? @const.vis
    name: (identifier) @const.name) @const.def)

;; static
(source_file
  (static_item
    (visibility_modifier)? @static.vis
    name: (identifier) @static.name) @static.def)

;; re_export — `pub use ...;` / `pub(crate) use ...;` at top level
;; The grammar models `pub use foo::bar as baz;` as
;; `(use_declaration (visibility_modifier) (_ ...))`. The modifier is
;; REQUIRED here, so a bare `use` never matches. The extractor then admits
;; exactly what the spec's `re_export.visibility` enum admits — `pub` and
;; `pub(crate)` (spec value `crate`) — through the adapter; `pub(super)`
;; and `pub(in ...)` drop (#104).
(source_file
  (use_declaration
    (visibility_modifier) @re_export.vis
    argument: (_) @re_export.target) @re_export.def)

;; macro_rules!
(source_file
  (macro_definition
    name: (identifier) @macro.name) @macro.def)

;; extern block — `extern "C" { ... }`
(source_file
  (foreign_mod_item) @extern_block.def)
