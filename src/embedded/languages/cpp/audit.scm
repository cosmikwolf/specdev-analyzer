;; C++ audit queries for tree-sitter.
;;
;; Captures the constructs module specs map onto in idiomatic C++:
;; class definitions, member functions, free functions, enums/enum-classes,
;; and namespaces (so qualified names can be reconstructed downstream).

;; Free functions (not in a class body)
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @fn.name
    parameters: (parameter_list) @fn.params)
  type: (_)? @fn.return_type) @fn.def

;; Class definitions (both struct and class are class_specifier in tree-sitter-cpp)
(class_specifier
  name: (type_identifier) @struct.name
  body: (field_declaration_list)? @struct.fields) @struct.def

;; Enum classes and plain enums
(enum_specifier
  name: (type_identifier) @enum.name
  body: (enumerator_list)? @enum.variants) @enum.def

;; Member function definitions declared inline in a class body — captured
;; the same way as free functions so the audit comparison can match
;; action-name → method.
(field_declaration_list
  (function_definition
    declarator: (function_declarator
      declarator: (field_identifier) @method.name
      parameters: (parameter_list) @method.params)
    type: (_)? @method.return_type) @method.def)

;; Namespace definitions — capture name so audit can report qualified locations.
(namespace_definition
  name: (namespace_identifier) @namespace.name
  body: (declaration_list) @namespace.body) @namespace.def
