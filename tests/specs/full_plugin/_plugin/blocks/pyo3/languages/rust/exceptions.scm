;; pyo3 exceptions, for the pyo3 rule pack (checks.toml).
;;
;; Every `create_exception!(module, Name, Base)` call, bare or
;; path-qualified (`pyo3::create_exception!`): the exception is the second
;; argument. The first `.` anchors the module to the first token, so the base
;; type (the third argument) is never taken for a name.
(
  (macro_invocation
    macro: [(identifier) @macro (scoped_identifier name: (identifier) @macro)]
    (token_tree . (identifier) . "," . (identifier) @exception.name))
  (#eq? @macro "create_exception")
)
