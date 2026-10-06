;; rtic tasks, for the rtic rule pack (checks.toml).
;;
;; One match per `key = value` argument of a `#[task(...)]` attribute, with
;; the name of the function the attribute decorates. List-valued arguments
;; (`shared = [..]`, `local = [..]`) are nested token trees and are not
;; matched here. Other attributes and comments may stand between the
;; attribute and the function.
(
  (attribute_item
    (attribute
      (identifier) @attr
      arguments: (token_tree
        (identifier) @arg.key
        .
        "="
        .
        [(identifier) (integer_literal)] @arg.value)))
  .
  [(attribute_item) (line_comment) (block_comment)]*
  .
  (function_item
    name: (identifier) @task.name)
  (#eq? @attr "task")
)

;; Every `#[task]` and `#[idle]` function, with or without arguments, so a
;; task that sets none is still found.
(
  (attribute_item
    (attribute
      (identifier) @attr))
  .
  [(attribute_item) (line_comment) (block_comment)]*
  .
  (function_item
    name: (identifier) @task.name)
  (#any-of? @attr "task" "idle")
)
