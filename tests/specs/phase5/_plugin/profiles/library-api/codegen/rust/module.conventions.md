# library-api: Rust module conventions

These conventions describe how a `library-api` profile module spec maps
to Rust source. They are language-and-architecture-only — framework
specifics come from the relevant block's `codegen/rust/README.md`,
layered on top.

## File layout

Library projects emit each module as `src/<name>.rs` for top-level
modules. Nested modules use `src/<parent>/<child>.rs`. The crate root
(`src/lib.rs`) declares each top-level module with its visibility:

```rust
pub mod public_module;        // visibility = "pub"
pub(crate) mod internal_helper; // visibility = "crate"
mod private_internal;         // visibility = "private"
```

For modules that share a parent (`src/<parent>/<child>.rs`), emit a
`src/<parent>.rs` (NOT `mod.rs`) that declares its children. Use
`mod.rs` only when the parent's logic itself spans multiple files and
needs a co-located implementation.

Do **not** emit a `mod.rs` for a parent that has only `pub mod` / `mod`
declarations and no logic of its own; use the file-per-module pattern.

## Visibility emission

`module.visibility` drives the `mod` keyword in the parent:

| visibility | crate root declaration       |
|------------|------------------------------|
| `pub`      | `pub mod <name>;`            |
| `crate`    | `pub(crate) mod <name>;`     |
| `private`  | `mod <name>;`                |

Within the module file, public items (the contract this module exposes)
are `pub`; helpers are bare. Do **not** emit `pub(crate)` on items
inside a `crate`-visible module — the module's own visibility already
constrains reach.

## Re-exports

`module.re_exports[]` becomes `pub use` (or `pub(crate) use`) statements
at the top of the module file:

```rust
// re_exports = [{ target: Inner, visibility: pub, alias: PublicName }]
pub use inner::Inner as PublicName;
```

Re-export rules:

- `visibility = "pub"` → `pub use`. Re-exported item is part of the
  crate's public API.
- `visibility = "crate"` → `pub(crate) use`. Internal use only.
- `alias` present → `as <alias>`. Otherwise the original name is
  preserved.
- `target` is a sibling module name; emit `target_name::TargetName` as
  the path. (When `target` is package-qualified `<package>::<Module>`,
  emit `package::module::Item`.)

The `re_exports_resolve` graph check verifies each `target` is a real
module before codegen runs.

Do **not** emit `pub use` for a `private` re-export — the schema
already forbids that combination.

## Feature gates

`module.feature_gates[]` lists cargo features that conditionally include
the module. Emission:

```rust
// feature_gates = ["alloc"]
#[cfg(feature = "alloc")]
pub mod alloc_dependent;

// feature_gates = ["alloc", "std"]
#[cfg(any(feature = "alloc", feature = "std"))]
pub mod heap_dependent;
```

An empty `feature_gates` (or omitted field) means the module is
unconditional — emit no `#[cfg(...)]` attribute.

The `module_feature_gates_resolve` graph check verifies every name is
declared in `architecture.toml`'s `[[features]]`.

Do **not** combine `feature_gates` with `#[cfg(target_*)]` predicates
in the same `cfg` — feature gating and target gating are orthogonal
concerns; emit them as separate `#[cfg(...)]` lines.

## Cross-module dependencies

`module.depends_on[]` does not directly emit code. Rust's module system
treats `use` statements as the unit of dependency, and `use` paths
appear naturally as the module's body is generated. The `depends_on`
declaration is for the spec's graph checks (`modules_acyclic`,
cross-module satisfies traceability).

When a module's body uses items from a sibling, emit `use crate::<sibling>::<Item>;`
at the top of the module file. Do **not** emit `use super::*;` — explicit
imports document dependency intent.

## API contract surface

For modules with `visibility = "pub"`, the public-API surface is what
appears under `pub` items inside the file plus the re-exports. The
`requirements.toml` `verified_by` field references these — typically as
`<module>::<Item>` or test names.

Public-API discipline:

- Emit `#[derive(Debug)]` on all public types unless documented
  otherwise.
- Emit `#[derive(Clone)]` on public types that are conceptually
  copy-cheap (POD-shaped).
- Do **not** emit `#[derive(Default)]` unless a `Default` impl is
  semantically meaningful — default-able types should be a deliberate
  decision, not a derive-by-default.
- Avoid `pub` on internal helpers, even if they're "obviously useful"
  to crate consumers; promote later if a real consumer asks.

## no_std and alloc

Library projects targeting `no_std` declare the constraint at crate
root:

```rust
#![cfg_attr(not(feature = "std"), no_std)]
```

This is the standard pattern: the library is `no_std` by default, and
opts into `std` via a feature. The `feature_gates` mechanism then lets
specific modules require `std` or `alloc`:

```rust
// In src/lib.rs:
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;
```

Do **not** emit `#![no_std]` unconditionally for a library that
declares a `std` feature in `architecture.toml` — the `cfg_attr` form
above is the right shape.

## Cargo.toml integration

`architecture.toml` `[[features]]` entries map directly to Cargo.toml's
`[features]` section:

```toml
[features]
default = ["std"]
std = []
alloc = []
serde = ["std"]
```

Generated `Cargo.toml` (or the manual one the project author maintains)
should reflect this exactly. Codegen does not own Cargo.toml in v1 —
maintain it manually, with a future enhancement to write it from the
spec.

Do **not** emit feature definitions in code (e.g. `#[cfg(feature = …)]`
for an undeclared feature). The schema check catches missing feature
names; runtime cargo will catch typos in declared-but-unused names.
