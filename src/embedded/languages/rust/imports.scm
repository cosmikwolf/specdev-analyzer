;; Rust import queries for tree-sitter (`check`'s internal edges).
;; `@import.source`: the path of every `use` declaration, as written
;; (`crate::a::b`, `super::x`, `crate::{a::f, b}`, `x as y`). The adapter
;; resolves it to a project file; anything that is not this crate's own
;; module (another crate, `std`, `core`) is external and ignored.

(use_declaration argument: (_) @import.source)

;; `@import.test_scope`: code compiled only for tests — a `mod` or `fn` under
;; `#[cfg(test)]` or `#[cfg(all(…, test, …))]`, or a `#[test]` fn. An import
;; inside it is not a dependency of the module. `cfg(not(test))` and
;; `cfg(any(test, …))` do not qualify: that code compiles in other builds.
((attribute_item (attribute (identifier) @_cfg arguments: (token_tree) @_args))
 .
 (attribute_item)*
 .
 [(mod_item) (function_item)] @import.test_scope
 (#eq? @_cfg "cfg")
 (#match? @_args "^\\(\\s*(test|all\\((.*,\\s*)?test(\\s*,.*)?\\))\\s*\\)$"))

((attribute_item (attribute (identifier) @_test))
 .
 (attribute_item)*
 .
 (function_item) @import.test_scope
 (#eq? @_test "test"))
