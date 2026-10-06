;; Test discovery for Dart (package:test / flutter_test) — `check`,
;; `report tests`. Captures as in the Rust file: @test, @test.name,
;; @test.pending, @group, @group.name. Titles are string literals; the
;; engine strips their quotes.

;; test('title', …) / testWidgets('title', …)
(call_expression
  function: (identifier) @_fn
  arguments: (arguments . (string_literal) @test.name)
  (#any-of? @_fn "test" "testWidgets")) @test

;; `skip:` named argument: pending.
(call_expression
  function: (identifier) @_fn
  arguments: (arguments (named_argument (label (identifier) @_label)))
  (#any-of? @_fn "test" "testWidgets")
  (#eq? @_label "skip")) @test @test.pending

;; group('title', …) encloses tests.
(call_expression
  function: (identifier) @_group
  arguments: (arguments . (string_literal) @group.name)
  (#eq? @_group "group")) @group
