;; Dart import queries for tree-sitter.
;; One capture, `@import.source`: the URI string of every `import` and
;; `export` directive. The analyzer maps `package:<name>/…` to `<name>`,
;; drops `dart:` URIs, relative URIs and the package's own name, and
;; compares the rest with what the project declares in `external_deps`.

(configurable_uri
  (uri (string_literal) @import.source))
