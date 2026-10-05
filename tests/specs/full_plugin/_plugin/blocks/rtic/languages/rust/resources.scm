;; rtic resources, for the rtic rule pack (checks.toml).
;;
;; A field of the app's `#[shared]` or `#[local]` struct. The attribute is the
;; resource's sharing. Other attributes and comments may stand between the
;; attribute and the struct.
(
  (attribute_item
    (attribute
      (identifier) @sharing))
  .
  [(attribute_item) (line_comment) (block_comment)]*
  .
  (struct_item
    body: (field_declaration_list
      (field_declaration
        name: (field_identifier) @resource.name)))
  (#any-of? @sharing "shared" "local")
)

;; An inline-initialized local on a task or `init`: `local = [name: Type = value]`.
;; A plain `local = [name]` refers to the struct and has no `:`, so it does not
;; match here.
(attribute
  arguments: (token_tree
    (identifier) @sharing
    .
    "="
    .
    (token_tree
      (identifier) @resource.name
      .
      ":"))
  (#eq? @sharing "local"))
