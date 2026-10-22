;; pyo3 classes, for the pyo3 rule pack (checks.toml).
;;
;; Every `#[pyclass]` struct or enum, with or without arguments, bare or
;; path-qualified. Other attributes (`#[derive(..)]`) and comments may stand
;; between the attribute and the item.
(
  (attribute_item
    (attribute [(identifier) @attr (scoped_identifier name: (identifier) @attr)]))
  .
  [(attribute_item) (line_comment) (block_comment)]*
  .
  [(struct_item name: (type_identifier) @class.name)
   (enum_item name: (type_identifier) @class.name)]
  (#eq? @attr "pyclass")
)
