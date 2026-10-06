;; One match per `key = value` argument of a `#[task(...)]` attribute,
;; paired with the name of the function the attribute decorates.
;; List-valued arguments (`shared = [..]`) are nested token_trees and
;; are not matched here.
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
  (function_item
    name: (identifier) @entity.key)
  (#eq? @attr "task")
)
