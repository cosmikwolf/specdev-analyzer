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

## The triad model

Requirements alone capture *what* the firmware must do, not *why* it
looks the way it does. Two sibling files complete the triad:

| File                        | Role                                                         |
|-----------------------------|--------------------------------------------------------------|
| `specs/requirements.toml`   | the **what** — intent and constraints the code must satisfy  |
| `specs/decisions.toml`      | the **how & why** — choices pinned over alternatives         |
| `specs/open_questions.toml` | the **known unknowns** — things the project hasn't answered  |

A decision (`DEC-NNN`) has a status of `accepted` or `superseded`, a
`title`/`text` body, and optional `context`, `consequences`,
`alternatives`, and `verification` prose. When a decision replaces an
earlier one, the accepted successor lists `supersedes = ["DEC-M"]` and
the superseded entry keeps its body as a reasoning trail. An open
question (`OQ-NNN`) has a status of `open`, `default_committed`, or
`closed`, plus `affects = [...]` naming the requirement ids the
uncertainty touches. The rest of the spec points at decisions through
typed references:

- `requirement.decided_by[]` — pins the *how* of a requirement
- `module.guided_by[]` — informs a module without pinning a requirement
- `architecture.project_decisions[]` — shapes the project as a whole

Open-question status gates code generation per module:

- `open` — hard-blocks `gen` for any module reachable through the
  question's `affects[]` (module → `satisfies[]` → affected
  requirement). `gen` aborts; no flag bypasses the gate.
- `default_committed` — passes the gate. The committed placeholder and
  its one-line `verify` plan are inlined into the codegen prompt, so
  the generated code knows it embeds a provisional assumption.
- `closed` — passes the gate; the resolved answer is inlined.

For example, an open question "what DMA buffer size does the SAI RX
path need?" with `affects = ["REQ-AUDIO-003"]` blocks `gen` for every
driver module satisfying that requirement until a decision either
answers it or commits a working default ("256 frames; verify against
worst-case ISR latency on target hardware"). Committing such a default
— downgrading the question to `default_committed` — is the in-spec
escape from the gate; use `/spec-question` for that flow and
`/spec-decide` to record decisions.

Decisions are not just bookkeeping: along the module → requirement →
`decided_by` walk, each accepted decision's `text` and `consequences`
are inlined into the codegen prompt, so decisions directly shape
generated code. Superseded decisions are skipped on the walk.

Cross-references between decisions and open questions are authored in
one direction by convention — prefer the question-side pointer
(`closed_by` / `default_via`) over the decision-side
`closes_questions[]` / `defaults_for[]`, since graph checks verify the
direction you author. Validation requires every triad reference to
resolve and supersession chains to be acyclic and orphan-free (each
superseded entry named by exactly one accepted entry's `supersedes[]`).
`validate` warns about gate-blocking open questions before `gen`
refuses, so you can clear them ahead of emission.

## Graph checks shipped

- `unique_qualified_names` — every entity has a unique qualified name (collisions across kinds are surfaced)
- `modules_acyclic` — module dependency graph has no cycles
- `layer_dependency` — no upward dependency in the layer ordering
- `connection_endpoints_resolve` — every architecture connection
  endpoint references an existing port on an existing module
- `requirement_ids_unique` — every Requirement id is unique
- `requirement_satisfies_resolves` — Requirement.satisfies references
  exist
- `module_satisfies_resolves` — Module.satisfies references exist

## Extending

Add blocks via `specdev inline <block>`. Bridges via
`specdev inline-bridge <a>+<b>`. Project-local bridges live under
`specs/specdev/reductions/bridges/local/` and are authored directly by
the project, not inlined.

To customize the layer ordering or add additional graph checks, edit the
project's `specs/specdev/domain.toml` directly — once bootstrapped, that
file is project-owned.
