;; Test discovery for C++ (GoogleTest, Catch2) — `check`, `report tests`.
;; Captures as in the Rust file, plus @test.suite: the engine names a test
;; `Suite.Name` when it has one.

;; TEST / TEST_F / TEST_P (Suite, Name) { … }
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @_macro
    parameters: (parameter_list
      .
      (parameter_declaration type: (type_identifier) @test.suite)
      .
      (parameter_declaration type: (type_identifier) @test.name)))
  (#any-of? @_macro "TEST" "TEST_F" "TEST_P")) @test

;; A `DISABLED_` prefix on the suite or the name: pending.
(function_definition
  declarator: (function_declarator
    declarator: (identifier) @_macro
    parameters: (parameter_list
      (parameter_declaration type: (type_identifier) @_part)))
  (#any-of? @_macro "TEST" "TEST_F" "TEST_P")
  (#match? @_part "^DISABLED_")) @test @test.pending

;; Catch2 TEST_CASE("title", "[tags]") { … }
(expression_statement
  (call_expression
    function: (identifier) @_macro
    arguments: (argument_list . (string_literal (string_content) @test.name)))
  (#eq? @_macro "TEST_CASE")) @test

;; A hidden tag (`[.]`, `[.name]`): pending.
(expression_statement
  (call_expression
    function: (identifier) @_macro
    arguments: (argument_list (string_literal (string_content) @_tag)))
  (#eq? @_macro "TEST_CASE")
  (#match? @_tag "\\[\\.")) @test @test.pending
