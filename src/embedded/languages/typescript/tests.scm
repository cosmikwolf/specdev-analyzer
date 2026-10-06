;; Test discovery for TypeScript (vitest / jest / Playwright) — `check`,
;; `report tests`. Captures as in the Rust file: @test, @test.name,
;; @test.unnamed, @test.pending, @group, @group.name.

;; test('title', …) / it('title', …), also as `.skip` / `.todo` / `.only`.
(call_expression
  function: [(identifier) @_fn
             (member_expression object: (identifier) @_fn)]
  arguments: (arguments . (string (string_fragment) @test.name))
  (#any-of? @_fn "test" "it")) @test

;; `.skip` and `.todo`: pending.
(call_expression
  function: (member_expression
    object: (identifier) @_fn
    property: (property_identifier) @_mod)
  (#any-of? @_fn "test" "it")
  (#any-of? @_mod "skip" "todo")) @test @test.pending

;; test.each([...])('title %s', …): the title is a template, so the cases
;; are counted without a name; the enclosing describe names them.
(call_expression
  function: (call_expression
    function: (member_expression
      object: (identifier) @_fn
      property: (property_identifier) @_each))
  (#any-of? @_fn "test" "it")
  (#eq? @_each "each")) @test @test.unnamed

;; describe('title', …) encloses tests.
(call_expression
  function: [(identifier) @_group
             (member_expression object: (identifier) @_group)]
  arguments: (arguments . (string (string_fragment) @group.name))
  (#eq? @_group "describe")) @group
