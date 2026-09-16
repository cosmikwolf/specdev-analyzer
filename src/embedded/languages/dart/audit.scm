;; Dart audit queries for tree-sitter.
;; Captures the top-level constructs the audit pipeline compares against
;; extracted module specs.

;; Top-level functions
(function_declaration
  signature: (function_signature
    name: (identifier) @fn.name
    parameters: (formal_parameter_list) @fn.params)) @fn.def

;; Classes (covers `abstract class` too — the modifier is not part of the match)
(class_declaration
  name: (identifier) @class.name
  body: (class_body) @class.body) @class.def

;; Mixins
(mixin_declaration
  name: (identifier) @mixin.name
  body: (class_body) @mixin.body) @mixin.def

;; Enums
(enum_declaration
  name: (identifier) @enum.name
  body: (enum_body) @enum.variants) @enum.def

;; Named extensions (unnamed extensions are not addressable symbols; skipped)
(extension_declaration
  name: (identifier) @extension.name
  body: (extension_body) @extension.body) @extension.def

;; Extension types
(extension_type_declaration
  name: (extension_type_name) @extension_type.name
  body: (class_body) @extension_type.body) @extension_type.def

;; Typedefs
(type_alias
  (type_identifier) @typedef.name) @typedef.def
