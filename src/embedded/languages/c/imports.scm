;; C import queries for tree-sitter (`check`'s internal edges).
;; `@import.source`: a quoted `#include "…"` path. A `<…>` include is a
;; system header and is not captured.

(preproc_include path: (string_literal (string_content) @import.source))
