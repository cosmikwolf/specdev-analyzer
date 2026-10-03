# rust-app — implementation guidance

Rules an agent follows when it writes or edits code in a `rust-app` package: a hosted Rust executable built as a
library plus a thin entry point. No check verifies these rules; `check` verifies the layers, the imports and the
tests, and review the rest.

## Two roots

- `src/lib.rs` holds every module the specs describe; tests import it.
- `src/main.rs` makes one call into an `entry` module (for example a `run` function) and nothing else. Startup logic
  lives in the library, where it can be tested.
- `std` is assumed: no `no_std`, no `alloc` crate declaration.

## Layers

- `entry` → `adapter` → `core` → `support`; dependencies point down only.
- `entry` wires the program: configuration, arguments, building the object graph.
- `adapter` touches the outside world: sockets, files, databases, GUI, clocks, processes.
- `core` holds the application's own types and rules and imports no framework, no I/O and no clock. When `core`
  needs the outside world, it declares a trait; an `adapter` implements it; `entry` connects them. This is what lets
  the application be tested without a runtime, a network or a window.
- `support` holds error types, logging setup and small helpers, and stays small.

## Files and visibility

- A top-level module is `src/<name>.rs`; a child is `src/<parent>/<child>.rs`, declared from `src/<parent>.rs`.
- `module.visibility` decides the declaration in the parent. In an application, `pub` means reachable by the crate's
  tests and `main.rs`; a module only its own layer uses is crate-visible.
- Import siblings by explicit path; no glob imports from the parent.
- Public types derive `Debug`; cheap value types derive `Clone`; `Default` only when a default means something.

## Contract surface and tests

- When a module keeps `items[]`, each public symbol is one item named by its full path, described in prose, never by
  signature. A `re_export` item is one `pub use`.
- A requirement's `verified_by` names the test by the identity that `specdev-analyzer report tests` shows. An
  unfinished test fails loudly and names the requirement it owes.
- `Cargo.toml` is the truth for dependencies and features; test-only crates are dev-dependencies. Every
  `cfg(feature = …)` names a declared feature.

## Tables

- A lookup table computed from a formula is generated in `build.rs`; a test checks anchor values.
