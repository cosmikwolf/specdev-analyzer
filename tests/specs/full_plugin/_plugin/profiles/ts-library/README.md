# ts-library profile

A profile for TypeScript packages published to npm whose load-bearing concern is the exported API surface: the
functions, classes and types other code imports.

## When to use this profile

Use `ts-library` for projects that:

- Ship as an npm package: ESM, `src/` compiled to `dist/` (JavaScript plus declarations).
- Have an exported API worth stating as a contract.
- Want type-checking, the tests and the build to run as gates.

An application (a Next.js app) uses `nextjs-app`.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, `toolchains` (gates and lockfiles per package manager), paired files |
| `schemas/module.schema.yaml` | per-module spec shape (optional `items[]`; no visibility field — `export` is the visibility) |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (project decisions) |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template` | bootstrap seeds |
| `templates/tsconfig.json.template` | written once: type-checks `src/` (sources and tests), `NodeNext`, strict, no emit |
| `templates/tsconfig.build.json.template` | written once: emits `dist/` with declarations and maps, test files left out |
| `templates/gitignore.template`, `templates/specdev-ci.yml.template` | written once: ignores, and the CI workflow (pnpm) |

`package.json` is the project's: dependencies, `exports` and `packageManager` live there, not in the spec.

## The tool and the gates

The project declares its package manager in `spec.toml`: `[project] toolchain = "pnpm"` (the default) or `"npm"`.
Each tool's gates, in order: `install` (frozen: `pnpm install --frozen-lockfile` / `npm ci`), `types` (`tsc
--noEmit`), `tests` (`vitest run`), `build` (`tsc -p tsconfig.build.json`). `check` reports an undeclared or unknown
tool, and a lockfile of another tool. Yarn is not listed: its install flags differ between major versions; add a
`yarn` entry to the project's `profile.yaml` copy when a project needs it.

## Module shape

```toml
name = "packet"
description = "Builds packets from words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "parse"
kind = "function"
description = "Builds a packet from its words; throws PacketError on a bad length."
satisfies = ["REQ-PKT-001"]
```

It lives in `src/packet.ts`. `items[]` is the contract surface: `specdev-analyzer items diff` compares it with the
exported names in the code, and `/spec-sync-items` reconciles them. A module with no code yet carries
`planned = true`.

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
- `decision_reference_superseded` (warning) — a module's `guided_by` or a requirement's `decided_by` names a superseded decision
- `open_question_closed_by_resolves` — `closed_by` names an existing decision
- `open_question_status_consistency` — `closed_by` / `default_via` name accepted decisions
- `implementation_unblocked` — the open-question gate, run by `check`: an `open` question reaches no module that has code

`check` adds the source rule, stale `planned` flags, imports against `depends_on`, requirement ↔ test
(vitest discovery), and the toolchain rules.

## Gates and CI

`specs/specdev/profile.yaml` declares the gates per package manager; the analyzer renders the declared tool's gates into
`scripts/specdev-gates.sh`, which runs `specdev-analyzer check` first and then each gate (`install`, `types`,
`tests`, `build`). CI runs the script. Bootstrap writes `.github/workflows/specdev.yml` once (yours afterwards); it
reads the repository variable `SPECDEV_ANALYZER_REPO` and the secret `SPECDEV_ANALYZER_TOKEN` to build the private
analyzer.

**Toolchain CI installs:** Node (LTS) and pnpm, and the stable Rust toolchain (cargo) to build
`specdev-analyzer`. typescript and vitest are dev dependencies.

CI installs are frozen: commit the lockfile (`pnpm-lock.yaml`); `pnpm install --frozen-lockfile` refuses to change
it.

## Context files

When a module needs reading that the structural spec cannot carry — a protocol's wire-format quirks, an algorithm
derivation — list it in the module's `context_files = ["docs/protocol-notes.md", ...]`. `specdev-analyzer context
--module` lists those paths for whoever edits the module; a missing file is `CONTEXT-001`. A choice belongs in a
decision, not a context file.

## Extending

Add blocks with `/spec-inline <block>` and bridges with `/spec-inline-bridge <a>+<b>`. Edit
`specs/specdev/domain.toml` to add or change graph checks; once bootstrapped, that file is project-owned.
