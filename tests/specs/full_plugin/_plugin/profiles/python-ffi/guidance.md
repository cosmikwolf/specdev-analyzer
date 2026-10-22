# python-ffi — implementation guidance

Rules an agent follows when it writes or edits code in a `python-ffi` package: an importable Python package built
around a compiled Rust extension. The Rust crate is its own specdev package (a Rust library profile with the Python
bindings block), in the directory `pyproject.toml` `[tool.maturin] manifest-path` names. `check` verifies the imports
and the tests, the `stubs` gate the stub against the built module, `items diff` the contract surface.

## The native module

- The extension is a private module of the package (`<package>._native`). That dotted name is `[tool.maturin]
  module-name` and `specs/architecture.toml` `[native] module`; its last segment is the Rust module's name and the
  crate's library name. One name in four places: change them together.
- The package's `__init__.py` re-exports the public names from the native module and from its Python modules. Users
  and tests import the package, never `_native`.
- The stub `_native.pyi` sits beside the extension and is the native module's spec source (its module spec's
  `source_file`). Change it in the same change as the Rust exports: the `stubs` gate fails on any difference.
- The stub states `__all__`, and marks each class `@final` unless the Rust side allows subclassing; the stubs gate
  checks both against the built module.
- With uv, list the package in `[tool.uv] reinstall-package`: uv does not see a change in the Rust crate and keeps
  the old extension, so every gate would check stale code.

## Layout and names

- `src/` layout: the import package is `src/<package>/`, and `[paths].source` names that directory. A leading `_`
  makes a module or a name private; private names are never items.
- Python modules hold what is better in Python (argument handling, iteration helpers, pure-Python fallbacks); the
  work stays in Rust. A Python wrapper never re-implements a native function.
- An import that only a type annotation needs goes under `if TYPE_CHECKING:`. Importing a module has no side effects.

## The contract surface

- Each public function, class, `UPPER_CASE` constant and type alias is one item; a method is `Class.member`. The
  native module's items come from its stub. Describe each in prose, never by signature.
- Every public function and method is fully type-annotated, and the package ships a `py.typed` marker.
- The native exceptions are the package's error types: re-export them; Python code raises them or subclasses of
  them, never a bare `Exception`.

## Dependencies, build and tests

- `pyproject.toml` is the truth for dependencies and the build backend (maturin); `Cargo.toml` for the crate's.
  Dev tools — test runner, type checker, linter, stub checker — sit in the `dev` dependency group.
- The tool `spec.toml` declares writes the lockfile; commit it, and leave no lockfile of another tool.
- Tests live in `tests/`, in files named `test_*.py`, and test the package as users import it. A requirement's
  `verified_by` names a test by the identity `specdev-analyzer report tests` shows.
- An unfinished test is marked skip with the requirement id as its reason; it never passes empty.
