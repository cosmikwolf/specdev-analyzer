# library-api profile

A profile for Rust library crates where the load-bearing concerns are
public-API surface, re-exports, and cargo features.

## When to use this profile

Use `library-api` for projects that:

- Ship as a Rust library crate (a `src/lib.rs` consumed by other
  crates, possibly published to crates.io).
- Have a meaningful public-API surface (`pub` types, `pub use`
  re-exports, `pub fn` items).
- Use cargo features to gate optional functionality (`std`, `alloc`,
  `serde`, etc.).
- Care about API stability and re-export discipline.

Use `firmware-layered` instead for embedded firmware projects with a
strict layered architecture (HAL → driver → middleware → application).

Use a future profile (none yet) for binary applications, build tools,
or projects whose primary concerns are neither layered hardware
abstraction nor library-API surface.

## Profile contents

| File                                       | Role                                                          |
|--------------------------------------------|---------------------------------------------------------------|
| `schemas/module.schema.yaml`               | per-module spec shape (visibility, items[], feature_gates)    |
| `schemas/architecture.schema.yaml`         | `specs/architecture.toml` shape (cargo features list)         |
| `schemas/requirements.schema.yaml`         | `specs/requirements.toml` shape (sources + reqs)              |
| `schemas/enums.schema.yaml`                | shared closed vocabularies (`Visibility`, `FeatureName`)      |
| `reductions/project.yaml`                  | TOML → universal entity/relation graph                        |
| `codegen/rust/module.conventions.md`       | Rust emission conventions for libraries                       |
| `spec.toml.template`                       | bootstrap-time `spec.toml` skeleton                           |
| `domain.toml.template`                     | bootstrap-time `domain.toml` skeleton + graph checks          |
| `architecture.toml.template`               | bootstrap-time empty features file with examples              |
| `requirements.toml.template`               | bootstrap-time empty requirements file with examples          |

## Module shape

A library-api `Module` is a public-API unit:

```toml
name = "Display"
visibility = "pub"
description = "Top-level rendering surface."
depends_on = ["Buffer", "Style"]
satisfies = ["REQ-API-001"]
feature_gates = ["alloc"]

[[items]]
name = "DisplayInner"
kind = "re_export"
description = "Convenience alias for Inner."
target = "inner::Inner"
visibility = "pub"
```

This produces `src/display.rs` (declared from `src/lib.rs` as `pub mod
display;` under `#[cfg(feature = "alloc")]`), with `pub use inner::Inner
as DisplayInner;` at the top.

## Graph checks shipped

- `unique_module_names` — every Module has a unique name
- `modules_acyclic` — module dependency graph has no cycles
- `feature_unique_names` — every cargo feature has a unique name
- `feature_dependencies_resolve` — feature `depends_on` references resolve
- `module_feature_gates_resolve` — every `feature_gates` entry names a declared feature
- `requirement_ids_unique` — every Requirement id is unique
- `requirement_satisfies_resolves` — Requirement.satisfies references exist
- `module_satisfies_resolves` — Module.satisfies references exist

## Deferred

- **`no-private-in-public-api`** — checks that a `pub` re-export's
  target traces (transitively) to a `pub` module. Expressible via
  `reachability` with constraint filters that the existing primitive
  doesn't yet support directly. Will land in a Phase 5.x or future
  evolution if dogfood reveals the gap matters.
- **Typed library-api API constraints** (MSRV, no_std/alloc support as
  enum fields). Currently expressed as free-form `requirement.text`.

## Extending

Add blocks via `specdev inline <block>` once the relevant blocks ship.
Bridges via `specdev inline-bridge <a>+<b>`. Edit `specs/specdev/domain.toml`
directly to add or modify graph checks; once bootstrapped, that file is
project-owned.
