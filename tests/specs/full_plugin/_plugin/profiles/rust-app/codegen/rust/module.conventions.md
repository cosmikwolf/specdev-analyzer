# rust-app: Rust module conventions

These conventions describe how a `rust-app` profile module spec maps
to Rust source. They are language-and-architecture-only — framework
specifics come from the relevant block's `codegen/rust/README.md`,
layered on top.

## Crate shape: a library plus a thin entry point

A `rust-app` crate has **two roots**, and only one of them is emitted
from module specs:

- **`src/lib.rs`** — the crate root for everything the specs describe.
  Every module spec lands under it. This is what tests import and what
  `cargo test` exercises.
- **`src/main.rs`** — the process entry point. Written **once**, on the
  first `cargo-toml` emit, from the profile's stub template, and never
  overwritten. It is hand-authored territory.

Consequence for codegen: **never emit into `src/main.rs`.** If a module
spec seems to describe process startup, it belongs to the `entry` layer
and still emits as an ordinary module under `src/lib.rs` (e.g.
`src/entry.rs` exposing `pub fn run() -> Result<()>`); `main.rs` then
calls it and does nothing else. Keeping `main` to a single call is what
makes startup logic testable at all.

## Layers

Every module declares a `layer`. Dependencies flow downward — `entry` →
`adapter` → `core` → `support` — and the `layer_dependency` graph check
rejects upward edges, so the constraint is enforced before codegen runs.
What each layer means for emission:

| layer | emits | must not |
| --- | --- | --- |
| `entry` | startup wiring, config/argument parsing, building the object graph | contain business rules worth testing on their own |
| `adapter` | anything touching the outside world — sockets, files, databases, GUI, clocks, other processes | contain logic that would be worth testing without its I/O |
| `core` | the application's own types and rules | import a framework, open a socket, touch the filesystem, or read a clock |
| `support` | error types, logging setup, small shared helpers | grow — a large `support` layer means something belongs in `core` |

The `core`-imports-nothing rule is the load-bearing one: it is what lets
the whole application be tested without a runtime, a network, or a
window. When a `core` module needs something from the outside, emit a
trait in `core` and its implementation in `adapter`, with `entry` wiring
the two together.

## File layout

Each module emits as `src/<name>.rs` for top-level modules. Nested
modules use `src/<parent>/<child>.rs`. The crate root (`src/lib.rs`)
declares each top-level module with its visibility:

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

## Item emission

`module.items[]` carries the module's contractual surface. The rule is one
line:

> An item is a **public, contractual code symbol, identified by its
> fully-qualified path, described in prose — never by signature.** Nesting
> determines the *path*, not whether something is an item.

A method is a function. A public function reached through `RateLimiter::`
is an item for the same reason a free `render` function is — it is public
contract that satisfies a requirement. So:

- `pub fn render` → item `render`
- inherent `impl RateLimiter { pub fn try_consume }` → item
  `RateLimiter::try_consume` (`kind = "fn"`)
- `pub const` / `pub type` in an inherent impl → `RateLimiter::CAPACITY` /
  `RateLimiter::Output`
- `pub struct RateLimiter` → item `RateLimiter`

The shape is **prose-first**: each item carries a `description` of what it
does, NOT a signature mirror of how it's typed. Never name parameters,
return types, struct fields, or generics — that is the source's job; the
spec describes the *contract*, the source describes the *implementation*.
The optional `signature_hint` field is a rare escape hatch for cases where
prose alone is insufficient (FFI declarations, macro contracts); avoid it
for ordinary functions and methods — reaching for it routinely turns the
spec back into a copy of the code.

**Inherent impls dissolve into their methods.** Do not author an item for
the `impl Type { … }` block itself — author one item per public member,
qualified by the type. Generics are normalized out of the prefix: write
`RateLimiter::new`, never `RateLimiter<C>::new`.

**Trait impls are one item; their method bodies are codegen-derived.** A
`impl Display for RateLimiter` is authored (when the conformance is
contractual) as a single `kind = "impl"` item named `Display for
RateLimiter`. Its `fmt` body is dictated by the trait, not your API, so it
is **not** a separate item — codegen writes it from the trait + the impl's
prose.

**Private / helper members are not items** — same as private free
functions. Only `pub` surface is contract.

Sparseness rule: every item except `kind = "re_export"` must carry
`satisfies` (≥1 requirement id). The schema enforces this — items without
traceability fail validation. `re_export` items are exempt because `pub
use` is inherently contractual. This applies per public method too, but
it is *per item, not per requirement*: many methods may share one coarse
requirement (a limiter's whole public surface can `satisfies =
["REQ-API-001"]`). Reach for requirement granularity, not signatures, when
the load feels heavy.

Example — a type and its public inherent methods:

```toml
[[items]]
name = "RateLimiter"
kind = "struct"
description = "Token-bucket limiter keyed by a monotonic clock."
satisfies = ["REQ-API-001"]

[[items]]
name = "RateLimiter::new"
kind = "fn"
description = "Construct a limiter with the given capacity and refill rate."
satisfies = ["REQ-API-001"]

[[items]]
name = "RateLimiter::try_consume"
kind = "fn"
description = "Attempt to take n tokens; report whether the budget allowed it."
satisfies = ["REQ-API-002"]

[[items]]
name = "Display for RateLimiter"
kind = "impl"
description = "Human-readable one-line summary of remaining budget."
satisfies = ["REQ-API-009"]
```

Notice no item names parameters, return types, or generics. A `pub`
symbol left out of `items[]` still compiles but shows as `in_source_only`
in `/spec-sync-items` — a correct signal that you have public surface the
spec doesn't account for. Reach a clean reconcile by either authoring the
member (with a `satisfies`) or making it private.

## Test items

An `[[items]]` entry with `test = true` is a **test**. It is a
`kind = "fn"` item that uses `verifies = [...]` (pointing at
`nature = "test"` requirements) in place of `satisfies`, and carries a
`test_kind` that drives where the test is emitted. The
`test_requirements_have_verifiers` graph check warns at validate time
when a `nature = "test"` requirement has no verifying test item — so the
test surface stays traceable the same way production code does.

```toml
[[items]]
name        = "roundtrip_serde"
kind        = "fn"
test        = true
test_kind   = "integration"
verifies    = ["REQ-TEST-001"]
description = "Round-trips a Value through serde_json and asserts equality."
```

### Emit location per `test_kind`

| `test_kind`   | Emit location                                                  |
|---------------|----------------------------------------------------------------|
| `unit`        | Inside `#[cfg(test)] mod tests { … }` in the declaring module's source file. Each test is a `#[test] fn <name>() { … }`. |
| `integration` | A `tests/<name>.rs` file at the crate root. The whole file is the integration test; emit `#[test] fn <name>() { … }` (plus any `use <crate>::…;` it needs against the crate's public API). |
| `doc`         | A `///` doc-comment example on the item named by `name` if such an item exists, otherwise on the module. Emit a fenced ` ```rust ` block whose body exercises the API and ends in an `assert!`/`assert_eq!`. |
| `bench`       | **No emission in v1** (enum slot reserved). Validate warns. |
| `fuzz`        | **No emission in v1** (enum slot reserved). Validate warns. |

The `verifies` ids resolve to `nature = "test"` requirements; the
codegen prompt's "Test items (verification context)" section inlines
each requirement's text, its `verified_by` hints, and any `decided_by`
decision's `verification` criteria. Use that context to write the test
body.

### Body emission

- **Write a working test** when the verification context carries enough
  to do so — the requirement text plus its verification hints / decision
  criteria are the spec for the test. This is the normal case; treat it
  like any other `gen` body (the same refuse-on-incomplete discipline
  applies).
- **Emit `todo!("verifies <ids>")`** — with the `verifies` ids joined
  into the message — only when the spec genuinely lacks the information
  to write the test. `cargo test` then compiles and the test fails loudly
  with a message pointing at the requirement(s) it owes. This is the test
  analogue of the standard `unimplemented!()`/`todo!()` gap marker — there
  is no separate "skeleton mode" (the skeleton feature was retired); test
  items emit as part of normal `gen` output.

Example (unit, co-located):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_serde() {
        // verifies REQ-TEST-001: round-trip serde equality
        let v = Value::sample();
        let json = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), v);
    }
}
```

Example (integration, `tests/golden_render.rs`, criteria insufficient):

```rust
#[test]
fn golden_render() {
    todo!("verifies REQ-TEST-602")
}
```

### Test-only dependencies

A test that needs a harness crate (`approx`, `proptest`, `criterion`)
declares it as a module `external_deps` entry with `test_only = true`;
codegen routes it to `[dev-dependencies]` rather than `[dependencies]`
(see "Cargo.toml integration"). Do not add test harness crates to the
regular dependency set.

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

## External crate dependencies

`module.external_deps[]` declares Cargo dependencies the module needs.
The union across every module in the library determines the full
dependency set the `gen` step writes to `Cargo.toml`. Each entry is a
[`CrateReference`](https://specdev.dev/schema/meta/v1/crate_reference.schema.yaml) —
`crate` + `version` are required; `source`, `features`,
`default_features`, and `optional` are optional.

`source` is a **tagged union** — pick one shape per entry:

```toml
# Plain crates.io dep — `source` omitted (the default).
[[external_deps]]
crate   = "serde"
version = "1"
features = ["derive"]

# Path dep on a sibling workspace member.
# Shape is `source = { path = "..." }`, NOT `source = "path", path = "..."`.
[[external_deps]]
crate   = "ym2414b-core"
version = "0.1"
source  = { path = "../ym2414b-core" }
features = ["std"]

# Workspace inheritance — render `<dep>.workspace = true`.
# Permitted alongside `optional = true`; package-field workspace
# inheritance is per-field via `architecture.toml`'s
# `[workspace_package]`.
[[external_deps]]
crate   = "tracing"
version = "0.1"
source  = "workspace"

# Git dep with a pinned revision (use `tag` or `branch` instead of
# `rev` if the upstream tags releases or you intentionally track a
# moving branch).
[[external_deps]]
crate   = "experimental-codec"
version = "0.0.0"
source  = { git = "https://github.com/example/experimental-codec", rev = "abc1234" }
```

Cargo features the module turns ON for an existing feature-gated dep
flow through `feature_gates` + the architecture's `[[features]]`
table — not through `external_deps`. Use `external_deps` for crates
the module needs unconditionally (or that the module's own feature
gate brings in).

`source = "crates-io"` is also accepted as an explicit "default
registry" marker; it is equivalent to omitting `source`.

## Contract surface

For modules with `visibility = "pub"`, the contract surface is what
appears under `pub` items inside the file plus the re-exports. The
`requirements.toml` `verified_by` field references these — typically as
`<module>::<Item>` or test names.

In an application, `pub` means "reachable by the crate's own tests and
by `src/main.rs`", not "published to other crates" — so the cost of a
wrong choice is lower than in a library, and the discipline below is
about keeping the crate legible rather than about semver. A module that
only its own layer uses should be `crate`, not `pub`.

Contract discipline:

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

## std is assumed

A `rust-app` crate builds a hosted executable, so `std` is always
available. Do **not** emit `#![no_std]`, `#![cfg_attr(…, no_std)]`, or
`extern crate alloc` — a `no_std` application binary is a different
project shape and belongs to a different profile.

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

The generated `Cargo.toml` reflects this exactly. Cargo.toml is OUTPUT:
the analyzer's `manifest` subcommand writes it deterministically from
the spec (architecture features + the union of module `external_deps`)
— never maintain it by hand; if a dep or feature is missing, edit the
spec and re-emit.

Do **not** emit feature definitions in code (e.g. `#[cfg(feature = …)]`
for an undeclared feature). The schema check catches missing feature
names; runtime cargo will catch typos in declared-but-unused names.
