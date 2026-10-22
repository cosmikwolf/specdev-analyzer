# firmware-layered profile

A profile for embedded firmware projects with a strict layered architecture:

```
application → middleware → driver → HAL → PAC
```

Modules declare their layer; dependencies flow downward only (same-layer allowed, upward forbidden). `check`
verifies both the spec (`layer_dependency`) and the code's imports (`internal_edges_declared`).

## When to use this profile

Use `firmware-layered` for projects that:

- Run on bare metal or on an embedded RTOS (embassy, RTIC, FreeRTOS).
- Have a clear architectural separation between hardware abstraction (PAC/HAL), peripheral drivers, application
  logic, and middleware.
- Target `no_std` with optional `alloc`.
- Author module specs covering hardware bindings, ports, actions, state machines, and concurrency/memory metadata.

Use `library-api` instead for a Rust library crate whose load-bearing concern is its public API surface.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, compile gates, paired files |
| `schemas/module.schema.yaml` | per-module spec shape |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (build target, port connections) |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies (`LayerKind`, `Identifier`) |
| `schemas/metadata.schema.yaml` | `metadata.{MemoryModel, ConcurrencyModel, …}` shapes |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template`, `architecture.toml.template` | bootstrap seeds |
| `templates/specdev-ci.yml.template` | the CI workflow, written once |

`Cargo.toml` and `.cargo/config.toml` are the project's: dependencies, features and build profiles live there, not
in the spec. The metadata fields (`isr_safe`, `blocking`, `metadata.*`) are checked by review and by the guidance —
no extraction can check them yet.

## Requirements, decisions, open questions

One file per entry, under `specs/requirements/`, `specs/decisions/` and `specs/open_questions/`:

| Ledger | Role |
|---|---|
| requirements | the **what** — intent and constraints the code must satisfy; `verified_by` names the tests |
| decisions | the **how & why** — choices pinned over alternatives (`accepted` or `superseded`) |
| open questions | the **known unknowns** — `open`, `default_committed` or `closed` |

A decision reaches a module through the module's `guided_by[]`, the `decided_by[]` of a requirement it satisfies,
or the workspace tier; `specdev-analyzer report decisions --module <M>` lists them for whoever edits the code. An
`open` question that reaches a module with code fails `check` (`implementation_unblocked`); the exits are a decision
that closes it or a committed default with a `verify` line. Use `/spec-decide` and `/spec-question`.

## Graph checks shipped

- `unique_qualified_names` — every entity has a unique qualified name
- `modules_acyclic` — module dependency graph has no cycles
- `layer_dependency` — no upward dependency in the layer ordering
- `connection_endpoints_resolve` — every architecture connection endpoint references an existing port
- `requirement_ids_unique`, `decision_ids_unique` — ids are unique
- `requirements_have_satisfiers` — every requirement is satisfied by a module (error; a `planned` one is exempt)
- `decision_supersedes_acyclic`, `decision_supersedes_chain_consistency` — supersession chains are sound
- `decision_reference_superseded` (warning) — a module's `guided_by` or a requirement's `decided_by` names a superseded decision
- `open_question_closed_by_resolves`, `open_question_status_consistency` — open-question references resolve
- `implementation_unblocked` — the open-question gate, run by `check`

`check` adds the source rule, stale `planned` flags, imports against `depends_on`, and requirement ↔ test.

## Gates and CI

`specs/specdev/profile.yaml` declares the compile gates; the analyzer renders them into `scripts/specdev-gates.sh`,
which runs `specdev-analyzer check` first and then each gate. CI runs the script. Bootstrap writes
`.github/workflows/specdev.yml` once (yours afterwards); it reads the repository variable `SPECDEV_ANALYZER_REPO` and
the secret `SPECDEV_ANALYZER_TOKEN` to build the private analyzer.

**Toolchain CI installs:** the stable Rust toolchain (cargo, rustup) plus the build target named in `[platform].target_triple` (the workflow adds it), and Rust again to build `specdev-analyzer`.

CI installs are frozen: commit the lockfile, or the CI install refuses to change it.

## Extending

Add blocks with `/spec-inline <block>` and bridges with `/spec-inline-bridge <a>+<b>`. Project-local bridges live
under `specs/specdev/reductions/bridges/local/` and are authored by the project.

To customize the layer ordering or add graph checks, edit the project's `specs/specdev/domain.toml` — once
bootstrapped, that file is project-owned.
