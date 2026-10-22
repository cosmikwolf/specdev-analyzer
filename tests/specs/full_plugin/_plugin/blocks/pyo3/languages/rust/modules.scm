;; pyo3 modules, for the pyo3 rule pack (checks.toml).
;;
;; Every `#[pymodule]` item: the declarative form on a `mod`, and the
;; function form on a `fn`. With or without arguments (`#[pymodule(name =
;; "..")]`), bare or path-qualified (`#[pyo3::pymodule]`). Other attributes
;; and comments may stand between the attribute and the item.
(
  (attribute_item
    (attribute [(identifier) @attr (scoped_identifier name: (identifier) @attr)]))
  .
  [(attribute_item) (line_comment) (block_comment)]*
  .
  [(mod_item name: (identifier) @module.name)
   (function_item name: (identifier) @module.name)]
  (#eq? @attr "pymodule")
)
