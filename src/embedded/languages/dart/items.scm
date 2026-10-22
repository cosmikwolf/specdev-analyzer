;; Dart items query for tree-sitter.
;;
;; Captures top-level declarations and public members per the items[]
;; schema (see `languages/dart/items.schema.yaml`). The analyzer's
;; `extract-items` subcommand walks these matches and produces
;; `ExtractedItem { kind, name, attributes[], span }` per
;; capture.
;;
;; Conventions:
;; - Each item kind has an `<kind>.def` capture (the whole node) and an
;;   `<kind>.name` capture (the identifier). The capture prefix IS the
;;   schema kind string — the extractor derives `kind` from it.
;; - Members of classes / mixins / extensions / extension types use the
;;   `inherent_<kind>.def` / `inherent_<kind>.name` convention with an
;;   `inherent.host` capture carrying the enclosing declaration's name;
;;   the extractor emits `<host>.<member>` qualified names.
;; - Dart has NO visibility nodes: privacy is the `_` name prefix and is
;;   filtered in the extractor (any `_`-prefixed segment → not an item),
;;   so no `.vis` captures appear in this file.
;; - Member patterns capture annotation names as `inherent.annotation`;
;;   the extractor drops `@override` members (an override implements an
;;   inherited contract — the contract lives on the declaring base, so
;;   an override is never new public surface). Annotations sit INSIDE
;;   `method_declaration` for concrete members but on `class_member`
;;   (as siblings of `declaration`) for abstract ones — both captured.
;; - Not captured in v1 (see the schema's description for rationale):
;;   constructors, fields, top-level `final`/`var` variables, `export`
;;   directives, unnamed extensions.
;;
;; Grammar ground truth (tree-sitter-dart 0.2.0, probed live):
;; - Class members are double-wrapped: `class_body > class_member >
;;   method_declaration signature: (method_signature > {function_signature
;;   | getter_signature | setter_signature})`. ABSTRACT members (no body)
;;   wrap in `declaration`, NOT `method_declaration` — both shapes are
;;   captured below or abstract-class surface silently vanishes.
;; - `type_alias` has no name field; the name is its first
;;   `type_identifier` child.
;; - `extension_type_declaration`'s name is wrapped in
;;   `extension_type_name`.
;; - Top-level `const` lives in `top_level_variable_declaration >
;;   static_final_declaration_list > static_final_declaration`.

;; ──────────────────────────────────────────────────────────────────
;; Top-level declarations
;; ──────────────────────────────────────────────────────────────────

;; function — top-level functions
(source_file
  (function_declaration
    signature: (function_signature
      name: (identifier) @function.name)) @function.def)

;; getter / setter — top-level property surface
(source_file
  (getter_declaration
    signature: (getter_signature
      name: (identifier) @getter.name)) @getter.def)

(source_file
  (setter_declaration
    signature: (setter_signature
      name: (identifier) @setter.name)) @setter.def)

;; class
(source_file
  (class_declaration
    name: (identifier) @class.name) @class.def)

;; mixin
(source_file
  (mixin_declaration
    name: (identifier) @mixin.name) @mixin.def)

;; enum
(source_file
  (enum_declaration
    name: (identifier) @enum.name) @enum.def)

;; extension — named only; an unnamed extension has no addressable name
;; and cannot be an item (the pattern requires the name field).
(source_file
  (extension_declaration
    name: (identifier) @extension.name) @extension.def)

;; extension type
(source_file
  (extension_type_declaration
    name: (extension_type_name
      (identifier) @extension_type.name)) @extension_type.def)

;; typedef — no name field in the grammar; first named child is the
;; alias' type_identifier.
(source_file
  (type_alias
    . (type_identifier) @typedef.name) @typedef.def)

;; const — top-level `const` declarations only. One match per declared
;; name (`const a = 1, b = 2;` yields two items).
(source_file
  (top_level_variable_declaration
    "const"
    (static_final_declaration_list
      (static_final_declaration
        name: (identifier) @const.name))) @const.def)

;; ──────────────────────────────────────────────────────────────────
;; Members — classes
;; ──────────────────────────────────────────────────────────────────

;; concrete method (incl. static)
(class_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (function_signature
            name: (identifier) @inherent_function.name))) @inherent_function.def)))

;; concrete getter
(class_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (getter_signature
            name: (identifier) @inherent_getter.name))) @inherent_getter.def)))

;; concrete setter
(class_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (setter_signature
            name: (identifier) @inherent_setter.name))) @inherent_setter.def)))

;; abstract method — wraps in `declaration`, not `method_declaration`
(class_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (annotation
        (identifier) @inherent.annotation)*
      (declaration
        (function_signature
          name: (identifier) @inherent_function.name)) @inherent_function.def)))

;; abstract getter
(class_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (annotation
        (identifier) @inherent.annotation)*
      (declaration
        (getter_signature
          name: (identifier) @inherent_getter.name)) @inherent_getter.def)))

;; abstract setter
(class_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (annotation
        (identifier) @inherent.annotation)*
      (declaration
        (setter_signature
          name: (identifier) @inherent_setter.name)) @inherent_setter.def)))

;; ──────────────────────────────────────────────────────────────────
;; Members — mixins
;; ──────────────────────────────────────────────────────────────────

(mixin_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (function_signature
            name: (identifier) @inherent_function.name))) @inherent_function.def)))

(mixin_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (getter_signature
            name: (identifier) @inherent_getter.name))) @inherent_getter.def)))

(mixin_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (setter_signature
            name: (identifier) @inherent_setter.name))) @inherent_setter.def)))

(mixin_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (annotation
        (identifier) @inherent.annotation)*
      (declaration
        (function_signature
          name: (identifier) @inherent_function.name)) @inherent_function.def)))

(mixin_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (annotation
        (identifier) @inherent.annotation)*
      (declaration
        (getter_signature
          name: (identifier) @inherent_getter.name)) @inherent_getter.def)))

(mixin_declaration
  name: (identifier) @inherent.host
  body: (class_body
    (class_member
      (annotation
        (identifier) @inherent.annotation)*
      (declaration
        (setter_signature
          name: (identifier) @inherent_setter.name)) @inherent_setter.def)))

;; ──────────────────────────────────────────────────────────────────
;; Members — extensions (body node is `extension_body`)
;; ──────────────────────────────────────────────────────────────────

(extension_declaration
  name: (identifier) @inherent.host
  body: (extension_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (function_signature
            name: (identifier) @inherent_function.name))) @inherent_function.def)))

(extension_declaration
  name: (identifier) @inherent.host
  body: (extension_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (getter_signature
            name: (identifier) @inherent_getter.name))) @inherent_getter.def)))

(extension_declaration
  name: (identifier) @inherent.host
  body: (extension_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (setter_signature
            name: (identifier) @inherent_setter.name))) @inherent_setter.def)))

;; ──────────────────────────────────────────────────────────────────
;; Members — extension types
;; ──────────────────────────────────────────────────────────────────

(extension_type_declaration
  name: (extension_type_name
    (identifier) @inherent.host)
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (function_signature
            name: (identifier) @inherent_function.name))) @inherent_function.def)))

(extension_type_declaration
  name: (extension_type_name
    (identifier) @inherent.host)
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (getter_signature
            name: (identifier) @inherent_getter.name))) @inherent_getter.def)))

(extension_type_declaration
  name: (extension_type_name
    (identifier) @inherent.host)
  body: (class_body
    (class_member
      (method_declaration
        (annotation
          (identifier) @inherent.annotation)*
        signature: (method_signature
          (setter_signature
            name: (identifier) @inherent_setter.name))) @inherent_setter.def)))
