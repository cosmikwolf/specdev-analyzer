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

## Item emission (Phase 14)

`module.items[]` carries the module's contractual surface entries — one
per top-level Rust construct that has spec significance (typically
because it satisfies a requirement or is a contractual public re-export).
The shape is **prose-first**: each item carries a `description` of what
it does, NOT a signature mirror of how it's typed. The optional
`signature_hint` field exists as an escape hatch for cases where prose
alone is insufficient (FFI declarations, macro contracts, generics-heavy
trait impls); avoid it for ordinary functions and types.

Sparseness rule: every item except `kind = "re_export"` must carry
`satisfies` (≥1 requirement id). The schema enforces this — items
without traceability fail validation. `re_export` items are exempt
because `pub use` is inherently contractual.

Example:

```toml
[[items]]
name = "render"
kind = "fn"
description = "Composites a frame from the layout tree onto the back buffer."
satisfies = ["REQ-API-001"]

[[items]]
name = "Style"
kind = "struct"
description = "Append-only collection of style overrides keyed by node id."
satisfies = ["REQ-API-014"]
```

Notice neither item names its parameters, return type, or struct fields
— that's source-level concern, captured in code. The spec describes the
*contract*; the source describes the *implementation*. When a `pub fn`
or `pub struct` lacks an items[] entry, codegen emits it but does NOT
trace it to a requirement; an in-source-only public item is permitted
(non-spec-significant API surface) but uncommon.

## Re-exports (via items[])

Re-exports are authored as `[[items]]` entries with `kind = "re_export"`.
Each becomes a `pub use` (or `pub(crate) use`) statement at the top of
the module file:

```rust
# [[items]]
# name = "PublicName"
# kind = "re_export"
# description = "Convenience alias for Inner."
# target = "inner::Inner"
# visibility = "pub"
pub use inner::Inner as PublicName;
```

Re-export rules:

- `visibility = "pub"` → `pub use`. Re-exported item is part of the
  crate's public API.
- `visibility = "crate"` → `pub(crate) use`. Internal use only.
- `name` differing from the trailing `target` segment → `as <name>`.
  Otherwise the original name is preserved.
- `target` is a path string. In-crate forms: `<sibling_module>::<Item>`.
  Cross-package forms: `<package>::<module>::<Item>` for multi-package
  projects.

`re_export` items are exempt from the items[] sparseness rule — the
inherently-contractual nature of a `pub use` makes `satisfies` optional
(but still permitted when an explicit traceability link is wanted).

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

## Const tables and `build.rs`

When a module's body computes a non-trivial lookup table — entries
derived from a formula (exponentials, log curves, ratio tables, packed
sample sets) rather than a handful of named constants — prefer
**`build.rs` table generation** over `const fn` evaluation:

```rust
// build.rs
use std::{env, fs, path::Path};

fn main() {
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("ratio_curve.rs");
    let mut entries = Vec::with_capacity(128);
    for i in 0..128u32 {
        let v = compute_ratio(i);
        entries.push(format!("    {v},"));
    }
    let body = format!(
        "pub static RATIO_CURVE: [u32; 128] = [\n{}\n];\n",
        entries.join("\n")
    );
    fs::write(out, body).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
```

```rust
// src/ratio_curve.rs
include!(concat!(env!("OUT_DIR"), "/ratio_curve.rs"));
```

Why over `const fn`:
- `Ord` and floating-point operations on associated `const`s are still
  unstable on stable Rust; `build.rs` sidesteps the entire trait-stability
  surface.
- Intermediate overflow on `u32` / `i32` arithmetic is easier to reason
  about in a regular `fn` than under `const`-context restrictions.
- Compile-time evaluation of large `const fn` table builders blows up
  build times on cold caches; `build.rs` runs once per build directory
  and re-runs only when its source changes.

Do **not** put the table values in the module spec — the spec records
the *shape* (length, element type, "computed from a formula") and points
the implementer at `build.rs`. Anchor-point fidelity (matching
documented values at e.g. index 0, 64, 127) belongs in a `#[test]` in
the module, not in the spec.

For trivial constants (single named values, small enums-as-numbers),
keep them inline as `pub const NAME: T = ...;`.

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
