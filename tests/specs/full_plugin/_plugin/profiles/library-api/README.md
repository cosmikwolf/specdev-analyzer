# library-api profile

A profile for Rust library crates whose load-bearing concern is the public API surface: public modules, re-exports,
the items other crates call.

## When to use this profile

Use `library-api` for projects that:

- Ship as a Rust library crate (a `src/lib.rs` consumed by other crates, possibly published to crates.io).
- Have a meaningful public API (`pub` types, `pub use` re-exports, `pub fn` items) worth stating as a contract.
- Care about API stability and re-export discipline.

Use `firmware-layered` for embedded firmware with a strict layered architecture, and `rust-app` for an executable.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, compile gate, paired files |
| `schemas/module.schema.yaml` | per-module spec shape (visibility, optional `items[]`) |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (project decisions) |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies (`Visibility`, `Identifier`) |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template` | bootstrap seeds |
| `templates/specdev-ci.yml.template` | the CI workflow, written once |

`Cargo.toml` is the project's: features, dependencies and the MSRV live there, not in the spec.

## Module shape

A library-api `Module` is a public-API unit:

```toml
name = "Display"
visibility = "pub"
description = "Top-level rendering surface."
depends_on = ["Buffer", "Style"]
satisfies = ["REQ-API-001"]

[[items]]
name = "DisplayInner"
kind = "re_export"
description = "Convenience alias for Inner."
target = "inner::Inner"
visibility = "pub"
```

It lives in `src/display.rs`, declared from `src/lib.rs` as `pub mod display;`. `items[]` is the contract surface:
`specdev-analyzer items diff` compares it with the public symbols in the code, and `/spec-sync-items` reconciles
them. A module with no code yet carries `planned = true`.

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
- `modules_acyclic` — the module dependency graph has no cycles
- `requirement_ids_unique` — requirement ids are unique
- `decision_ids_unique` — decision ids are unique
- `requirements_have_satisfiers` — every requirement is satisfied by a module (error; a `planned` requirement is exempt)
- `decision_supersedes_acyclic` — supersession chains have no cycle
- `decision_supersedes_chain_consistency` — every superseded decision is named by exactly one successor, and every decision named in `supersedes` is `superseded`
- `open_question_closed_by_resolves` — `closed_by` names an existing decision
- `open_question_status_consistency` — `closed_by` / `default_via` name accepted decisions
- `implementation_unblocked` — the open-question gate, run by `check`: an `open` question reaches no module that has code

`check` adds the source rule, stale `planned` flags, imports against `depends_on`, and requirement ↔ test.

## Gates and CI

`specs/specdev/profile.yaml` declares the compile gates; the analyzer renders them into `scripts/specdev-gates.sh`,
which runs `specdev-analyzer check` first and then each gate. CI runs the script. Bootstrap writes
`.github/workflows/specdev.yml` once (yours afterwards); it reads the repository variable `SPECDEV_ANALYZER_REPO` and
the secret `SPECDEV_ANALYZER_TOKEN` to build the private analyzer.

**Toolchain CI installs:** the stable Rust toolchain (cargo), which also builds `specdev-analyzer`.

CI installs are frozen: commit the lockfile, or the CI install refuses to change it.

## Deferred

- **`no-private-in-public-api`** — a `pub` re-export's target traces (transitively) to a `pub` module. Needs
  constraint filters on `reachability` that the primitive does not support yet.
- **Typed API constraints** (MSRV, no_std/alloc support as fields). Expressed as requirement text today; `Cargo.toml`
  holds the MSRV itself.

## Context files

When a module needs reading that the structural spec cannot carry — a protocol's wire-format quirks, an algorithm
derivation — list it in the module's `context_files = ["docs/protocol-notes.md", ...]`. `specdev-analyzer context
--module` lists those paths for whoever edits the module; a missing file is `CONTEXT-001`. A choice belongs in a
decision, not a context file.

## Extending

Add blocks with `/spec-inline <block>` and bridges with `/spec-inline-bridge <a>+<b>`. Edit
`specs/specdev/domain.toml` to add or change graph checks; once bootstrapped, that file is project-owned.
