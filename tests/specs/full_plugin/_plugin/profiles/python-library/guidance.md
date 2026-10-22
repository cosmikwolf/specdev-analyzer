# python-library — implementation guidance

Rules an agent follows when it writes or edits code in a `python-library` package: an importable Python package
whose public surface is its contract. No check verifies these rules; `check` verifies the imports and the tests,
`items diff` the contract surface, and review the rest.

## Layout and names

- `src/` layout: the import package is `src/<package>/`, and `[paths].source` names that directory. A module is
  `<name>.py`; a subpackage is a directory with an `__init__.py`. A module in a subpackage sets `source_file`.
- A leading `_` makes a module or a name private. Private names are never items and never imported from outside
  their package.
- The package's `__init__.py` is its public API: it imports the public names from their modules and holds no logic.
- Inside the package, import project modules with relative imports; tests import the package by its name.
- An import that only a type annotation needs goes under `if TYPE_CHECKING:` — `check` does not count it as a
  dependency, so it cannot hide a real layering edge.
- Importing a module has no side effects: no I/O, no global state set up, no work beyond definitions.

## The contract surface

- When the module keeps `items[]`, each public function, class, `UPPER_CASE` constant and type alias is one item; a
  public method or class constant is `Class.member`. Describe each in prose — never by signature or annotation.
  Dunder methods are covered by their class item.
- Every public function and method is fully type-annotated, and the package ships a `py.typed` marker; the type
  checker gate runs on the package.
- Errors: the package defines one base exception; public code raises its subclasses, never a bare `Exception`, and
  never catches everything to hide a failure.
- A public symbol missing from `items[]` shows as source-only in the items sync: author it with a `satisfies`, or
  make it private.

## Dependencies and the tool

- `pyproject.toml` is the truth for dependencies, extras and the supported Python versions. Runtime dependencies
  state a lower bound; dev tools (test runner, type checker, linter) sit in the `dev` dependency group.
- The tool `spec.toml` declares writes the lockfile; commit it, and leave no lockfile of another tool.
- An optional dependency belongs to an extra. Import it inside the function that needs it, and raise a clear error
  naming the extra when it is missing.

## Tests

- Tests live in `tests/`, in files named `test_*.py`; a test class is named `Test*`. A requirement's `verified_by`
  names a test by the identity `specdev-analyzer report tests` shows.
- An unfinished test is marked skip with the requirement id as its reason, so it shows as pending; it never passes
  empty.
