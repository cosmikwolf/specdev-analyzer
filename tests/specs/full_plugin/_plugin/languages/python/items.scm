;; Python items query for tree-sitter.
;;
;; Captures module-level items per the items[] schema (see
;; `languages/python/items.schema.yaml`); the analyzer's `extract-items`
;; walks these matches and produces `ExtractedItem { kind, name,
;; attributes[], span }` per capture.
;;
;; Conventions:
;; - Each kind has a `<kind>.def` capture (the whole node) and a
;;   `<kind>.name` capture (the identifier).
;; - Members of a module-level class use the `inherent_<kind>.*` captures
;;   plus `inherent.host` (the class name); the item is `<Class>.<name>`.
;; - Every pattern is anchored under `(module ...)`: nested functions,
;;   nested classes and declarations inside `if` / `try` blocks are never
;;   items.
;; - There is no `.vis` capture: privacy is the `_` prefix, applied by the
;;   adapter's name rule (a member of a `_Private` class is private too).
;; - A decorated definition is matched through `decorated_definition`; the
;;   decorators are not items.

;; fn — module-level `def` / `async def`, decorated or not
(module
  [(function_definition name: (identifier) @fn.name) @fn.def
   (decorated_definition definition: (function_definition name: (identifier) @fn.name)) @fn.def])

;; class — module-level, decorated or not
(module
  [(class_definition name: (identifier) @class.name) @class.def
   (decorated_definition definition: (class_definition name: (identifier) @class.name)) @class.def])

;; const — module-level assignment to an UPPER_CASE name (annotated or not)
(module
  (expression_statement
    (assignment left: (identifier) @const.name) @const.def)
  (#match? @const.name "^[A-Z][A-Z0-9_]*$"))

;; type — `type X = ...` and `X: TypeAlias = ...`
(module
  (type_alias_statement left: (type (identifier) @type.name)) @type.def)
(module
  (expression_statement
    (assignment left: (identifier) @type.name type: (type (identifier) @_ann)) @type.def)
  (#eq? @_ann "TypeAlias"))

;; members — methods of a module-level class (class and method decorated or not)
(module
  [(class_definition name: (identifier) @inherent.host
     body: (block [(function_definition name: (identifier) @inherent_fn.name) @inherent_fn.def
                   (decorated_definition definition: (function_definition name: (identifier) @inherent_fn.name)) @inherent_fn.def]))
   (decorated_definition definition: (class_definition name: (identifier) @inherent.host
     body: (block [(function_definition name: (identifier) @inherent_fn.name) @inherent_fn.def
                   (decorated_definition definition: (function_definition name: (identifier) @inherent_fn.name)) @inherent_fn.def])))])

;; members — class-level UPPER_CASE constants (enum members)
(module
  [(class_definition name: (identifier) @inherent.host
     body: (block (expression_statement (assignment left: (identifier) @inherent_const.name) @inherent_const.def)))
   (decorated_definition definition: (class_definition name: (identifier) @inherent.host
     body: (block (expression_statement (assignment left: (identifier) @inherent_const.name) @inherent_const.def))))]
  (#match? @inherent_const.name "^[A-Z][A-Z0-9_]*$"))
