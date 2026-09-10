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

## Triad model

Requirements never travel alone. Three sibling artifacts under
`specs/` carry the project's intent, choices, and unknowns:

| File                  | Role                                                              |
|-----------------------|-------------------------------------------------------------------|
| `requirements.toml`   | the WHAT — intent and constraints, traced to sources              |
| `decisions.toml`      | the HOW & WHY — implementation choices pinned over alternatives   |
| `open_questions.toml` | the KNOWN UNKNOWNS — things the project hasn't answered yet       |

A decision (`DEC-001`, "Use `thiserror` for the public error type")
carries a `status` of `accepted` or `superseded`. Supersession keeps
both bodies as a reasoning trail: the successor lists
`supersedes = ["DEC-003"]`, the old entry flips to `superseded`, and
codegen prompt walks follow only `accepted` decisions. A decision can
also resolve open questions via `closes_questions[]` / `defaults_for[]`
— author ONE direction per cross-reference (convention: prefer the
question-side `closed_by` / `default_via` pointers; see the comment in
`decisions.toml.template`).

Typed references wire decisions into the rest of the spec:

- `requirement.decided_by[]` — pins the *how* of a requirement
- `module.guided_by[]` — informs a module without pinning a requirement
- `architecture.project_decisions[]` — shapes the project as a whole

An open question (`OQ-001`, "Should the error type be an enum or a
boxed trait object?") lists the requirement ids it `affects` and moves
through three statuses:

- `open` — no committed answer. **Hard-blocks codegen**: `gen` aborts
  for any module whose `satisfies[]` reaches this question through its
  `affects[]`. No CLI flag bypasses the gate.
- `default_committed` — a decision committed a working default
  (`default_via`), with a one-line `verify` falsification plan. Codegen
  proceeds; the placeholder and `verify` text are inlined into the
  prompt for affected modules.
- `closed` — a decision resolved it (`closed_by`). Codegen proceeds;
  the prompt inlines the resolved answer. The entry persists as audit
  trail.

The in-spec escape from the gate is downgrading a question to
`default_committed` — committing a provisional default through a
decision. Use `/spec-question` for that flow and `/spec-decide` to
record decisions.

Decisions aren't passive documentation: along the module → requirement
→ `decided_by` walk, each accepted decision's rationale text is inlined
into the codegen prompt, so a recorded choice (say, a builder-pattern
constructor API over config structs) directly shapes the generated
code.

Validation backs all of this. `validate` checks that supersession
chains are acyclic and consistent (every superseded entry is named by
exactly one accepted successor), that every cross-reference resolves,
and it warns about gate-blocking open questions before `gen` refuses.

## Graph checks shipped

- `unique_qualified_names` — every entity has a unique qualified name (collisions across kinds are surfaced)
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
  doesn't yet support directly. Will land if dogfood reveals the gap
  matters.
- **Typed library-api API constraints** (MSRV, no_std/alloc support as
  enum fields). Currently expressed as free-form `requirement.text`.

## Out-of-band context (escape hatch)

When a module needs prose that doesn't fit the structural spec — a DSP
algorithm derivation, a protocol's wire-format quirks, a citations-list
the codegen agent must consult — declare it via the per-module
`context_files = ["docs/dsp-notes.md", ...]` field. The analyzer's
`prompt --module <Name>` step inlines each file's contents alongside
the schema-derived prose when assembling the codegen prompt. Paths are
project-relative; missing files raise `CONTEXT-001` at validate time.

Use sparingly: structural facts (item lists, dependencies, satisfies)
belong in the schema; `context_files` is for the irreducibly prose
context that schema can't carry.

## Extending

Add blocks via `specdev inline <block>` once the relevant blocks ship.
Bridges via `specdev inline-bridge <a>+<b>`. Edit `specs/specdev/domain.toml`
directly to add or modify graph checks; once bootstrapped, that file is
project-owned.
