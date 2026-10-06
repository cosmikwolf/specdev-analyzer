;; Dart import queries for tree-sitter (`check`'s internal edges).
;; `@import.source`: the URI of every `import` directive; `@import.reexport`:
;; the URI of every `export` directive (a barrel library is followed through
;; it). The adapter resolves `package:<own package>/…` and relative URIs;
;; `dart:` and other packages are external and ignored. The engine strips
;; the URI's quotes.

(library_import
  (import_specification uri: (configurable_uri (uri (string_literal) @import.source))))

(library_export
  uri: (configurable_uri (uri (string_literal) @import.reexport)))
