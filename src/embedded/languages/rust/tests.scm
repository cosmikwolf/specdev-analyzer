;; Test discovery for Rust (`check`, `report tests`).
;;
;; Captures the engine reads:
;;   @test          the node that is one test (merged across patterns)
;;   @test.name     its static name; absent with @test.unnamed (counted only)
;;   @test.pending  present: the test is switched off
;;   @group / @group.name   an enclosing container a `verified_by` may name
;;   @test_macro    the name of a macro, defined in the file, that makes tests
;;   @test.macro    the macro a test node calls; it counts only when that
;;                  macro is a @test_macro of the same file
;;
;; A test is a fn with a test attribute anywhere in the attribute run
;; directly above it: `#[test]`, `#[tokio::test]` (any `<path>::test`),
;; `#[rstest]`.
(
  (attribute_item (attribute [(identifier) (scoped_identifier)] @_attr))
  .
  (attribute_item)*
  .
  (function_item name: (identifier) @test.name) @test
  (#match? @_attr "^(test|rstest|([A-Za-z_][A-Za-z0-9_]*::)*test)$")
)

;; `#[ignore]` in the same attribute run: pending.
(
  (attribute_item (attribute (identifier) @_ignore)) @test.pending
  .
  (attribute_item)*
  .
  (function_item) @test
  (#eq? @_ignore "ignore")
)

;; `proptest! { ... }`: its tests sit in an unparsed token tree, so they
;; are counted without a name; the enclosing `mod` names them.
((macro_invocation
   macro: [(identifier) (scoped_identifier)] @_macro) @test @test.unnamed
 (#match? @_macro "(^|::)proptest$"))

;; A `macro_rules!` macro whose rule body holds `#[test]` makes tests.
(macro_definition
  name: (identifier) @test_macro
  (macro_rule right: (token_tree (token_tree (identifier) @_t)))
  (#eq? @_t "test"))

;; A call of such a macro is one test, named by its first identifier after any
;; attributes (`m!(name, …)`, `m!(#[ignore] name, …)`). A macro that takes its
;; name elsewhere is not detected.
((macro_invocation
   macro: (identifier) @test.macro
   (token_tree . (identifier) @test.name)) @test)
((macro_invocation
   macro: (identifier) @test.macro
   (token_tree (token_tree) . (identifier) @test.name)) @test)

;; `#[ignore …]` among the call's attributes: pending.
((macro_invocation
   macro: (identifier) @test.macro
   (token_tree (token_tree . (identifier) @_ignore) @test.pending)) @test
 (#eq? @_ignore "ignore"))

;; `mod name { ... }` encloses tests.
(mod_item name: (identifier) @group.name) @group
