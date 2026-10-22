;; Test discovery for Python (`check`, `report tests`): pytest's rules.
;;
;; Captures the engine reads:
;;   @test          the node that is one test (merged across patterns)
;;   @test.name     its name
;;   @test.pending  present: the test is switched off (a `skip` marker)
;;   @group / @group.name   an enclosing test class a `verified_by` may name
;;
;; A test is a `test*` function at module level, or a `test*` method of a
;; `Test*` class or of a `unittest.TestCase` subclass. pytest ignores a
;; `test*` method of any other class, and so does this query. The adapter
;; reads only test files (`test_*.py`, `*_test.py`). A parametrized test is
;; one test.

;; Module-level test functions, decorated or not.
(module (function_definition name: (identifier) @test.name (#match? @test.name "^test")) @test)
(module (decorated_definition
  definition: (function_definition name: (identifier) @test.name (#match? @test.name "^test")) @test))

;; Test methods: direct members of a `Test*` class.
(class_definition name: (identifier) @_cls (#match? @_cls "^Test")
  body: (block [(function_definition name: (identifier) @test.name) @test
                (decorated_definition definition: (function_definition name: (identifier) @test.name) @test)])
  (#match? @test.name "^test"))

;; Test methods: direct members of a `TestCase` subclass.
(class_definition superclasses: (argument_list) @_bases (#match? @_bases "TestCase")
  body: (block [(function_definition name: (identifier) @test.name) @test
                (decorated_definition definition: (function_definition name: (identifier) @test.name) @test)])
  (#match? @test.name "^test"))

;; Pending: a `skip` marker (`@pytest.mark.skip`, `@pytest.mark.skipif`,
;; `@unittest.skip`) on a test, in the same three places as above.
(module (decorated_definition (decorator) @_dec (#match? @_dec "skip")
  definition: (function_definition name: (identifier) @test.name (#match? @test.name "^test")) @test @test.pending))
(class_definition name: (identifier) @_cls (#match? @_cls "^Test")
  body: (block (decorated_definition (decorator) @_dec (#match? @_dec "skip")
    definition: (function_definition name: (identifier) @test.name (#match? @test.name "^test")) @test @test.pending)))
(class_definition superclasses: (argument_list) @_bases (#match? @_bases "TestCase")
  body: (block (decorated_definition (decorator) @_dec (#match? @_dec "skip")
    definition: (function_definition name: (identifier) @test.name (#match? @test.name "^test")) @test @test.pending)))

;; Groups: `Test*` classes and `TestCase` subclasses.
(class_definition name: (identifier) @group.name (#match? @group.name "^Test")) @group
(class_definition name: (identifier) @group.name
  superclasses: (argument_list) @_b (#match? @_b "TestCase")) @group
