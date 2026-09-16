;; C audit queries for tree-sitter.

;; Functions (definitions only — declarations handled at the declaration level).
;; We match on the declarator/parameters and ignore the return type field; some
;; grammars surface `type:` as optional/variadic which breaks strict matching.
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @fn.name
    parameters: (parameter_list) @fn.params)) @fn.def

;; Structs (named)
(struct_specifier
  name: (type_identifier) @struct.name
  body: (field_declaration_list)? @struct.fields) @struct.def

;; Enums (named)
(enum_specifier
  name: (type_identifier) @enum.name
  body: (enumerator_list) @enum.variants) @enum.def

;; Typedefs to struct — capture the typedef name too.
(type_definition
  type: (struct_specifier
    name: (type_identifier)? @typedef.struct_tag
    body: (field_declaration_list)? @typedef.struct_fields)
  declarator: (type_identifier) @typedef.name) @typedef.def
