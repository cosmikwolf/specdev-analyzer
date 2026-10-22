# `pyo3` block

Models the Python-visible surface of a Rust crate built with [PyO3](https://pyo3.rs): its extension modules,
classes, functions and exception types. Contributes a `[pyo3]` table to `Module`, an entity per entry
(`pyo3.PyModule`, `pyo3.Class`, `pyo3.Function`, `pyo3.Exception`), a rule pack whose code checks report an entry
the code does not have, and the implementation guidance for the module form, names, errors and the build.

## When to use

Inline it into the Rust crate that holds the `#[pymodule]` — usually a `library-api` crate with `cdylib` + `rlib`
crate types. The Python package that imports the built module is a separate specdev project with the `python-ffi`
profile, whose `stubs` gate holds its `.pyi` stub to the built module; maturin builds the wheel from that package.

## What it contributes

```toml
[pyo3]
modules = [{ name = "_native", description = "The extension module the Python package imports." }]
classes = [{ name = "Packet" }]
functions = [{ name = "parse" }]
exceptions = [{ name = "PacketError" }]
```

Names only: the Rust compiler and the Python package's tests hold the signatures and types. Each entry is the Rust
identifier, which is also the Python name (the guidance forbids renames). An entry not built yet takes
`planned = true`.

| Check | What it reports |
|---|---|
| `pack_entity_in_code` (modules) | no `#[pymodule]` `mod` or `fn` with that name |
| `pack_entity_in_code` (classes) | no `#[pyclass]` struct or enum with that name |
| `pack_entity_in_code` (functions) | no `#[pyfunction]` fn with that name |
| `pack_entity_in_code` (exceptions) | no `create_exception!` with that name as its second argument |

The queries were measured on code that compiles against PyO3 (declarative and function-form modules, a struct and
an enum class, a function with a `#[pyo3(signature)]` attribute and one inside a module, two exceptions with a bare
and a path base type): each found exactly the declared names and nothing else.

## Sharp test

- **Leg 1 — concreteness:** one crate, PyO3. Not "Python bindings" in general (cffi, uniffi and a C ABI are other
  blocks if a project needs them).
- **Leg 2 — rule-pack completeness:** each entry's only field is `name`, and a code check holds it to the code;
  `description` is prose. Everything the schema cannot state is a guidance rule.
- **Leg 3 — no cross-block reach:** the guidance names no other block. maturin appears only here, as the build
  tool PyO3 documents; the Python package side belongs to its own profile.

## Deferred

- **Export coverage.** A check that each entry is exported from a module (`#[pymodule_export]` or defined inside the
  `mod`) — a guidance rule today; the code check sees only that the item exists.
- **Class flags** (`frozen`, `eq`, `subclass`) as fields. The guidance says when to use `frozen`; promote when a
  project needs the spec to hold it.
- **Type stub generation** (`experimental-inspect`). Still in development upstream; the Python package keeps a
  hand-written `.pyi`.
