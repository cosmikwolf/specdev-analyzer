;; Rust import queries for tree-sitter (`check`'s internal edges).
;; `@import.source`: the path of every `use` declaration, as written
;; (`crate::a::b`, `super::x`, `crate::{a::f, b}`, `x as y`). The adapter
;; resolves it to a project file; anything that is not this crate's own
;; module (another crate, `std`, `core`) is external and ignored.

(use_declaration argument: (_) @import.source)

;; A path rooted at `crate`, `super` or `self` in an expression, a pattern or
;; a type is a dependency too, with no `use` line (`crate::led::X`,
;; `&crate::led::Led`). The paths nested in one capture start at the same byte
;; (`crate::led` inside `crate::led::X`); the analyzer keeps the longest. A
;; path inside a macro's arguments is a token tree, not a path: not seen.
((scoped_identifier) @import.source
 (#match? @import.source "^(crate|super|self)::"))

((scoped_type_identifier) @import.source
 (#match? @import.source "^(crate|super|self)::"))

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
