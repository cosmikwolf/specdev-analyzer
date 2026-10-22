;; Python audit queries for tree-sitter.
;; Captures the module-level constructs the audit pipeline compares against
;; extracted module specs.

;; Module-level functions
(module
  (function_definition
    name: (identifier) @fn.name
    parameters: (parameters) @fn.params) @fn.def)

;; Module-level classes
(module
  (class_definition
    name: (identifier) @class.name
    body: (block) @class.body) @class.def)
