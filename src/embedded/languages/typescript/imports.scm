;; TypeScript import queries for tree-sitter.
;; One capture, `@import.source`: the text inside the module specifier of
;; every static import, re-export and dynamic `import("…")`. The analyzer
;; maps each specifier to a package name (dropping relative, alias and
;; `node:` specifiers) and compares the set with what the module declares
;; in `external_deps`. Same query for `.ts` and `.tsx`.

;; import x from "pkg"; import "pkg"; import type { T } from "pkg"
(import_statement
  source: (string (string_fragment) @import.source))

;; export { x } from "pkg"; export * from "pkg"
(export_statement
  source: (string (string_fragment) @import.source))

;; await import("pkg")
(call_expression
  function: (import)
  arguments: (arguments (string (string_fragment) @import.source)))
