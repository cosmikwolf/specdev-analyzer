;; TypeScript audit queries for tree-sitter.
;; Captures the top-level constructs the audit pipeline compares against
;; extracted module specs. One query text serves both grammars the language
;; spans (`.ts` → typescript, `.tsx` → tsx): the node names are identical,
;; and the analyzer compiles the query once per grammar.
;;
;; `export` is not part of any match: an exported declaration is the same
;; node wrapped in an `export_statement`, so every pattern below matches
;; exported and module-private declarations alike. Visibility filtering is
;; the items vocabulary's job (`items.scm`), not the audit's.

;; Top-level functions (overload signatures are `function_signature` nodes
;; and are deliberately not matched — one item per implementation)
(function_declaration
  name: (identifier) @fn.name
  parameters: (formal_parameters) @fn.params) @fn.def

;; Classes
(class_declaration
  name: (type_identifier) @class.name
  body: (class_body) @class.body) @class.def

;; Abstract classes (a distinct node in the grammar, not a modifier)
(abstract_class_declaration
  name: (type_identifier) @abstract_class.name
  body: (class_body) @abstract_class.body) @abstract_class.def

;; Interfaces
(interface_declaration
  name: (type_identifier) @interface.name
  body: (interface_body) @interface.body) @interface.def

;; Type aliases
(type_alias_declaration
  name: (type_identifier) @type_alias.name) @type_alias.def

;; Enums
(enum_declaration
  name: (identifier) @enum.name
  body: (enum_body) @enum.variants) @enum.def

;; Top-level `const` / `let` bindings with a plain identifier name
;; (destructuring bindings are not addressable symbols; skipped)
(program
  (lexical_declaration
    (variable_declarator
      name: (identifier) @binding.name)) @binding.def)

;; Exported top-level bindings — the same node one level deeper
(program
  (export_statement
    declaration: (lexical_declaration
      (variable_declarator
        name: (identifier) @binding.name)) @binding.def))
