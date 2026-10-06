;; Test discovery for C (Unity) — `check`, `report tests`. Captures as in
;; the Rust file.

;; A function whose name starts with `test` is a test.
(function_definition
  declarator: (function_declarator declarator: (identifier) @test.name)
  (#match? @test.name "^test")) @test

;; `TEST_IGNORE()` in its body: pending.
(function_definition
  declarator: (function_declarator declarator: (identifier) @_name)
  body: (compound_statement
    (expression_statement (call_expression function: (identifier) @_ignore)))
  (#match? @_name "^test")
  (#eq? @_ignore "TEST_IGNORE")) @test @test.pending
