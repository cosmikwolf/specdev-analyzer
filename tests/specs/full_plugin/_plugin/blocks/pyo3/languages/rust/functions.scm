;; pyo3 functions, for the pyo3 rule pack (checks.toml).
;;
;; Every `#[pyfunction]` fn, at file level or inside a `#[pymodule] mod`,
;; with or without arguments, bare or path-qualified. Other attributes
;; (`#[pyo3(signature = ..)]`) and comments may stand between the attribute
;; and the fn, on either side.
(
  (attribute_item
    (attribute [(identifier) @attr (scoped_identifier name: (identifier) @attr)]))
  .
  [(attribute_item) (line_comment) (block_comment)]*
  .
  (function_item name: (identifier) @function.name)
  (#eq? @attr "pyfunction")
)
