;; TypeScript items query for tree-sitter.
;;
;; Captures exported top-level declarations and public members of exported
;; classes per the items[] schema (see `languages/typescript/items.schema.yaml`).
;; The analyzer's `extract-items` subcommand walks these matches and
;; produces `ExtractedItem { kind, name, attributes[], span }`
;; per capture. One query text serves both grammars the language spans
;; (`.ts` → typescript, `.tsx` → tsx); the analyzer compiles it per grammar.
;;
;; Conventions:
;; - Each item kind has a `<kind>.def` capture (the whole node) and a
;;   `<kind>.name` capture (the identifier). The capture prefix IS the
;;   schema kind string — the extractor derives `kind` from it.
;; - Visibility is `export`. Every top-level pattern is wrapped in an
;;   `export_statement` and captures the `export` keyword as `<kind>.vis`;
;;   a declaration that is not exported never matches. The adapter treats
;;   a top-level `.vis` of `export` as public.
;; - Members of exported classes use the `inherent_<kind>.def` /
;;   `inherent_<kind>.name` convention with an `inherent.host` capture
;;   carrying the class name; the extractor emits `<Class>.<member>`.
;;   Member patterns capture `(accessibility_modifier)? @inherent_<kind>.vis`
;;   — an absent modifier means public; `private` / `protected` mean not
;;   an item. The adapter reads the capture prefix to tell the two rules
;;   apart. Members of a NON-exported class never match: the host pattern
;;   requires the enclosing `export_statement`.
;; - `#private` members bind `private_property_identifier`, not
;;   `property_identifier`, so every member pattern below excludes them by
;;   node type. The adapter's `#`-prefix name rule is belt-and-braces.
;; - `constructor` is an ordinary `method_definition` in the grammar; the
;;   `#not-eq?` predicate drops it. Getters and setters are
;;   `method_definition` nodes with a `get` / `set` token; the plain method
;;   pattern excludes them with a text predicate on the node's own text.
;; - Overload signatures are `function_signature` / `method_signature`
;;   nodes and are not matched — one item per implementation.
;; - Not captured in v1 (see the schema's description): constructors,
;;   fields / properties, interface members, `export default <expression>`
;;   of an anonymous value, `declare` blocks, `namespace` bodies (every
;;   top-level pattern is anchored under `program`, so an `export` inside
;;   a namespace never matches), and destructuring NESTED more than one
;;   level (`export const { a: { b } } = x`). A destructured export at one
;;   level IS captured — one item per bound identifier; see the `const`
;;   section.
;;
;; Grammar ground truth (tree-sitter-typescript 0.23.2, read from
;; `node-types.json` and probed live):
;; - `export_statement` fields: `declaration`, `source`, `value`; children
;;   `export_clause` (named re-exports) or `namespace_export`
;;   (`* as ns`). A bare `export * from "x"` has NO `export_clause` — its
;;   shape is `export_statement > "*" + source: (string)`.
;; - `export default class X` / `export default function f` still place
;;   the declaration under the same `export_statement`; the patterns below
;;   do not name the field so both forms match.
;; - Class members: `class_body > method_definition` (fields `name`,
;;   `parameters`, `body`; child `accessibility_modifier`). Abstract
;;   members are `abstract_method_signature`. Fields are
;;   `public_field_definition` and are not matched.
;; - `abstract_class_declaration` is a distinct node, not a modifier.

;; ──────────────────────────────────────────────────────────────────
;; Top-level exported declarations
;; ──────────────────────────────────────────────────────────────────

;; function — plain, async, and generator functions
(program
  (export_statement
    "export" @function.vis
    (function_declaration
      name: (identifier) @function.name)) @function.def)

(program
  (export_statement
    "export" @function.vis
    (generator_function_declaration
      name: (identifier) @function.name)) @function.def)

;; class — concrete and abstract
(program
  (export_statement
    "export" @class.vis
    (class_declaration
      name: (type_identifier) @class.name)) @class.def)

(program
  (export_statement
    "export" @class.vis
    (abstract_class_declaration
      name: (type_identifier) @class.name)) @class.def)

;; interface
(program
  (export_statement
    "export" @interface.vis
    (interface_declaration
      name: (type_identifier) @interface.name)) @interface.def)

;; type — type aliases
(program
  (export_statement
    "export" @type.vis
    (type_alias_declaration
      name: (type_identifier) @type.name)) @type.def)

;; enum
(program
  (export_statement
    "export" @enum.vis
    (enum_declaration
      name: (identifier) @enum.name)) @enum.def)

;; const — exported `const` / `let` bindings.
;; One match per declarator (`export const a = 1, b = 2;` yields two).
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (identifier) @const.name))) @const.def)

;; const — DESTRUCTURED exports. One item per bound identifier.
;;
;; A destructured export exports its bindings exactly as a plain one does,
;; and this is not an exotic shape the tree can avoid: the `next-auth`
;; block gives `export const { handlers, auth, signIn, signOut } =
;; NextAuth({…})` as THE configuration form, the `next-auth+prisma` bridge
;; repeats it with the adapter, and the block's route file is
;; `export const { GET, POST } = handlers;` under "Never add a third
;; export". While these were unmatched, every item on such a module sat in
;; `in_spec_only` drift that no source could ever clear.
;;
;; The bound name sits on a DIFFERENT node per binding form, so each form
;; needs its own pattern — tree-sitter has no descendant axis, so one
;; pattern cannot cover them. Probed against tree-sitter-typescript
;; 0.23.2, the seven forms at one level of destructuring are below.
;;
;; Boundary, and it is deliberate: ONE level. `{ a: { b } }` nests an
;; `object_pattern` inside a `pair_pattern`, and each further level would
;; need its own pattern again. No shipped unit prescribes a nested export,
;; and the schema says so rather than leaving it to be discovered.

;; { a, b }
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (object_pattern
          (shorthand_property_identifier_pattern) @const.name)))) @const.def)

;; { a: renamed } — the item is the local binding, never the source key
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (object_pattern
          (pair_pattern
            value: (identifier) @const.name))))) @const.def)

;; { a = 1 }
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (object_pattern
          (object_assignment_pattern
            left: (shorthand_property_identifier_pattern) @const.name))))) @const.def)

;; { a, ...rest }
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (object_pattern
          (rest_pattern
            (identifier) @const.name))))) @const.def)

;; [a, b]
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (array_pattern
          (identifier) @const.name)))) @const.def)

;; [a, ...rest]
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (array_pattern
          (rest_pattern
            (identifier) @const.name))))) @const.def)

;; [a = 1]
(program
  (export_statement
    "export" @const.vis
    (lexical_declaration
      (variable_declarator
        name: (array_pattern
          (assignment_pattern
            left: (identifier) @const.name))))) @const.def)

;; ──────────────────────────────────────────────────────────────────
;; Re-exports — no `.vis`: every form is `export` by construction
;; ──────────────────────────────────────────────────────────────────

;; export { a, b as c } / export { a } from "./x" / export type { A }
;; One item per specifier; the name is the alias when present.
(program
  (export_statement
    (export_clause
      (export_specifier) @re_export.target @re_export.def)))

;; export * from "./barrel" — no export_clause; the item is named by the
;; module specifier (quotes stripped by the extractor).
(program
  (export_statement
    "*"
    source: (string) @re_export.target) @re_export.def)

;; export * as ns from "./x" — the item is the namespace name.
(program
  (export_statement
    (namespace_export
      (identifier) @re_export.target)) @re_export.def)

;; ──────────────────────────────────────────────────────────────────
;; Members — public members of exported classes
;; ──────────────────────────────────────────────────────────────────

;; concrete method (incl. static, async, generator) — not a constructor,
;; not an accessor
(program
  (export_statement
    (class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (method_definition
          (accessibility_modifier)? @inherent_function.vis
          name: (property_identifier) @inherent_function.name) @inherent_function.def
        (#not-eq? @inherent_function.name "constructor")
        (#not-match? @inherent_function.def "(^|\\s)(get|set)\\s")))))

;; getter
(program
  (export_statement
    (class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (method_definition
          (accessibility_modifier)? @inherent_getter.vis
          "get"
          name: (property_identifier) @inherent_getter.name) @inherent_getter.def))))

;; setter
(program
  (export_statement
    (class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (method_definition
          (accessibility_modifier)? @inherent_setter.vis
          "set"
          name: (property_identifier) @inherent_setter.name) @inherent_setter.def))))

;; abstract classes — the same three shapes, plus abstract member
;; signatures (no body; a distinct node)
(program
  (export_statement
    (abstract_class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (method_definition
          (accessibility_modifier)? @inherent_function.vis
          name: (property_identifier) @inherent_function.name) @inherent_function.def
        (#not-eq? @inherent_function.name "constructor")
        (#not-match? @inherent_function.def "(^|\\s)(get|set)\\s")))))

(program
  (export_statement
    (abstract_class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (method_definition
          (accessibility_modifier)? @inherent_getter.vis
          "get"
          name: (property_identifier) @inherent_getter.name) @inherent_getter.def))))

(program
  (export_statement
    (abstract_class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (method_definition
          (accessibility_modifier)? @inherent_setter.vis
          "set"
          name: (property_identifier) @inherent_setter.name) @inherent_setter.def))))

(program
  (export_statement
    (abstract_class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (abstract_method_signature
          (accessibility_modifier)? @inherent_function.vis
          name: (property_identifier) @inherent_function.name) @inherent_function.def
        (#not-match? @inherent_function.def "(^|\\s)(get|set)\\s")))))

(program
  (export_statement
    (abstract_class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (abstract_method_signature
          (accessibility_modifier)? @inherent_getter.vis
          "get"
          name: (property_identifier) @inherent_getter.name) @inherent_getter.def))))

(program
  (export_statement
    (abstract_class_declaration
      name: (type_identifier) @inherent.host
      body: (class_body
        (abstract_method_signature
          (accessibility_modifier)? @inherent_setter.vis
          "set"
          name: (property_identifier) @inherent_setter.name) @inherent_setter.def))))
