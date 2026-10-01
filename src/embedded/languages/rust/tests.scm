;; Test discovery for Rust (`check`, `report tests`).
;;
;; Captures the engine reads:
;;   @test          the node that is one test (merged across patterns)
;;   @test.name     its static name; absent with @test.unnamed (counted only)
;;   @test.pending  present: the test is switched off
;;   @group / @group.name   an enclosing container a `verified_by` may name
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

;; `mod name { ... }` encloses tests.
(mod_item name: (identifier) @group.name) @group
