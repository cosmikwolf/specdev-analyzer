;; Rust audit queries for tree-sitter.
;; Captures the top-level constructs the audit pipeline compares against
;; extracted module specs.

;; Functions / methods
(function_item
  name: (identifier) @fn.name
  parameters: (parameters) @fn.params
  return_type: (_)? @fn.return_type) @fn.def

;; Structs
(struct_item
  name: (type_identifier) @struct.name
  body: (field_declaration_list)? @struct.fields) @struct.def

;; Enums
(enum_item
  name: (type_identifier) @enum.name
  body: (enum_variant_list) @enum.variants) @enum.def

;; Impl blocks
(impl_item
  type: (_) @impl.type
  body: (declaration_list) @impl.body) @impl.def

;; Traits
(trait_item
  name: (type_identifier) @trait.name
  body: (declaration_list) @trait.body) @trait.def
