;; TypeScript import queries for tree-sitter (`check`'s internal edges).
;; `@import.source`: the module specifier of every static import and dynamic
;; `import("…")`. `@import.reexport`: the specifier of `export … from "…"`,
;; which a barrel file (an `index.ts` that belongs to no module) is followed
;; through. The adapter resolves relative and `tsconfig` alias specifiers;
;; a package specifier is external and ignored. Same query for `.ts` / `.tsx`.

;; import x from "./a"; import "./a"; import type { T } from "@/a"
(import_statement
  source: (string (string_fragment) @import.source))

;; export { x } from "./b"; export * from "./b"
(export_statement
  source: (string (string_fragment) @import.reexport))

;; await import("./a")
(call_expression
  function: (import)
  arguments: (arguments (string (string_fragment) @import.source)))
