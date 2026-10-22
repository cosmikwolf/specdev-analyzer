# pyo3 — implementation guidance

Rules for a Rust crate that exposes a Python API through PyO3. The `[pyo3]` table lists the modules, classes,
functions and exceptions Python sees, by Rust name; the rule pack's code checks (`pack_entity_in_code`, per entry
kind) report an entry the code does not have. Check the PyO3 version in the project's manifest: these rules follow
its current API.

## Names

- The Python name is the Rust name: no `name = ".."` argument on `#[pyclass]`, `#[pymodule]` or `#[pyo3]`. A
  rename makes the spec entry, the code and the Python import disagree.
- An extension module's name is the import name, and it must match the built library's file name, or the import
  fails. Keep the native module private to its package (a leading `_`) and re-export from the Python side.

## Modules

- Declare a module with `#[pymodule]` on a `mod` (the declarative form); add each class, function and exception with
  `#[pymodule_export] use`, or define a `#[pyfunction]` inside the module. Inside the `mod`, import the prelude again:
  the parent's imports do not reach it.
- Every `[pyo3]` entry is exported from a module; an entry that no module exports is not reachable from Python even
  though it compiles.
- A module allows free-threaded Python unless it says `gil_used = true`. Say it when the code relies on the
  interpreter lock to protect shared state.

## Classes and functions

- A value type that Python must not mutate is `#[pyclass(frozen)]`; mutable classes are borrowed at run time, and a
  conflicting borrow raises instead of failing to compile.
- Methods are in a `#[pymethods]` block of their class and are not listed in the table: the class entry covers them.
- Long-running Rust work releases the interpreter lock (`Python::detach`) and calls `Python::check_signals` at a
  steady rate, so Ctrl-C can cancel it.

## Errors

- A public function returns `PyResult`. Each failure Python can see is a `[pyo3]` exception (`create_exception!`),
  raised with its `new_err`; map Rust error types to them in one place.
- Never let a panic reach Python on purpose: it surfaces as `PanicException`, which callers cannot handle as a
  domain error.

## Build

- The crate type is `cdylib` plus `rlib`: the `cdylib` is the extension, the `rlib` lets `cargo test` and other
  crates link it.
- Do not enable the old `extension-module` feature; the Python build tool tells PyO3 not to link `libpython` when it
  builds the extension, and with the feature `cargo test` fails to link.
- To ship one wheel for several Python versions, enable the limited-API feature that names the oldest supported
  Python (`abi3-py3X`). Free-threaded Python cannot load an `abi3` wheel; the `abi3t` features cover both.
