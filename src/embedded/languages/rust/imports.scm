;; Rust import queries for tree-sitter (`check`'s internal edges).
;; `@import.source`: the path of every `use` declaration, as written
;; (`crate::a::b`, `super::x`, `crate::{a::f, b}`, `x as y`). The adapter
;; resolves it to a project file; anything that is not this crate's own
;; module (another crate, `std`, `core`) is external and ignored.

(use_declaration argument: (_) @import.source)
