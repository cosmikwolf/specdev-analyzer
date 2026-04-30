# firmware-layered profile

A profile for embedded firmware projects with a strict layered architecture:

```
application → middleware → driver → HAL → PAC
```

Modules declare their layer; dependencies flow downward only (same-layer
allowed, upward forbidden). Emitted code lives under `src/<layer>/` with
one file per module.

## When to use this profile

Use `firmware-layered` for projects that:

- Run on bare metal or on an embedded RTOS (embassy, RTIC, FreeRTOS).
- Have a clear architectural separation between hardware abstraction
  (PAC/HAL), peripheral drivers, application logic, and middleware.
- Target `no_std` with optional `alloc`.
- Author module specs covering hardware bindings, ports, actions, state
  machines, and concurrency/memory metadata.

Use `library-api` instead if you're shipping a Rust library crate where
the load-bearing concerns are public-API surface, re-exports, and feature
flags rather than hardware layering.

## Profile contents

| File                                       | Role                                                   |
|--------------------------------------------|--------------------------------------------------------|
| `schemas/module.schema.yaml`               | per-module spec shape                                  |
| `schemas/architecture.schema.yaml`         | `specs/architecture.toml` shape (port connections)     |
| `schemas/requirements.schema.yaml`         | `specs/requirements.toml` shape (sources + reqs)       |
| `schemas/enums.schema.yaml`                | shared closed vocabularies (`LayerKind`, `Identifier`) |
| `schemas/metadata.schema.yaml`             | `metadata.{MemoryModel, ConcurrencyModel, …}` shapes   |
| `reductions/project.yaml`                  | TOML → universal entity/relation graph                 |
| `codegen/rust/module.conventions.md`       | Rust emission conventions                              |
| `spec.toml.template`                       | bootstrap-time `spec.toml` skeleton                    |
| `domain.toml.template`                     | bootstrap-time `domain.toml` skeleton + graph checks   |
| `architecture.toml.template`               | bootstrap-time empty architecture file with examples   |
| `requirements.toml.template`               | bootstrap-time empty requirements file with examples   |

## Graph checks shipped

- `unique_module_names` — every Module has a unique name
- `modules_acyclic` — module dependency graph has no cycles
- `layer_dependency` — no upward dependency in the layer ordering
- `connection_endpoints_resolve` — every architecture connection
  endpoint references an existing port on an existing module
- `requirement_ids_unique` — every Requirement id is unique
- `requirement_satisfies_resolves` — Requirement.satisfies references
  exist
- `module_satisfies_resolves` — Module.satisfies references exist

## Extending

Add blocks via `specdev inline <block>` once the relevant blocks ship
(Phase 6+). Bridges via `specdev inline-bridge <a>+<b>` (Phase 7+).
Project-local bridges live under `specs/specdev/reductions/bridges/local/`
and are authored directly by the project, not inlined.

To customize the layer ordering or add additional graph checks, edit the
project's `specs/specdev/domain.toml` directly — once bootstrapped, that
file is project-owned.
