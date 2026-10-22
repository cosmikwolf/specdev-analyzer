;; Python import queries for tree-sitter (`check`'s internal edges).
;;
;;   @import.source      the module of each import: `a.b`, `.a`, `..a`, `.`
;;   @import.member      a name imported `from` a module (`from . import x`,
;;                       `from pkg import sub`). It is an import only when
;;                       `<source>.<member>` resolves to a project file (a
;;                       submodule); otherwise it is a name and is ignored.
;;   @import.test_scope  code that never runs: an `if TYPE_CHECKING:` block.
;;                       An import inside it is not a dependency.
;;
;; The adapter resolves an absolute module against the source root and a
;; relative one against the importing file's package. A module the project
;; does not contain is external (the manifest's business). `from __future__`
;; is a separate node and is not captured.

;; import a.b / import a.b as c
(import_statement name: (dotted_name) @import.source)
(import_statement name: (aliased_import name: (dotted_name) @import.source))

;; from a.b import c / from .a import b / from . import b
(import_from_statement module_name: [(dotted_name) (relative_import)] @import.source)

;; the imported names, which may be submodules
(import_from_statement
  module_name: [(dotted_name) (relative_import)] @import.source
  name: [(dotted_name) @import.member
         (aliased_import name: (dotted_name) @import.member)])

;; `if TYPE_CHECKING:` (also `typing.TYPE_CHECKING`)
(if_statement
  condition: [(identifier) (attribute)] @_cond
  consequence: (block) @import.test_scope
  (#match? @_cond "(^|\\.)TYPE_CHECKING$"))
