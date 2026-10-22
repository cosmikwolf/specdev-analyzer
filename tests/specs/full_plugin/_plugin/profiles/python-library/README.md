# python-library profile

A profile for importable Python packages whose load-bearing concern is the public API surface: the modules, classes
and functions other code imports.

## When to use this profile

Use `python-library` for projects that:

- Ship as an importable Python package (a `src/<package>/` tree, possibly published to PyPI).
- Have a public API worth stating as a contract: public functions, classes, constants, type aliases.
- Want the tests, the type checker and the linter to run as gates.

A package whose native half is a Rust extension (PyO3, maturin) uses `python-ffi` instead, once it ships.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, `toolchains` (gates and lockfiles per tool), paired files |
| `schemas/module.schema.yaml` | per-module spec shape (optional `items[]`; no visibility field — the `_` prefix is the visibility) |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (project decisions) |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies (`Identifier`, `ModuleRef`, `RequirementId`) |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template` | bootstrap seeds |
| `templates/specdev-ci.yml.template` | the CI workflow, written once (for uv) |

`pyproject.toml` is the project's: dependencies, extras and the supported Python versions live there, not in the
spec.

## The tool

The project declares its tool in `spec.toml`: `[project] toolchain = "uv"` (the default), `"poetry"` or `"pip"`.
The profile gives each tool's gates and lockfile; `specdev-analyzer check` reports an undeclared or unknown tool,
and a lockfile of another tool. The gates run the same three jobs for every tool, through `python -m`: `pytest`,
`pyright` (type check) and `ruff check` (lint). With `pip`, start the gate script inside an activated virtual
environment that has the `dev` group installed. `languages/python/build.md` gives every command for every tool.

## Module shape

A python-library `Module` is one Python module:

```toml
name = "packet"
description = "Parses and builds UMP packets."
depends_on = ["words"]
satisfies = ["REQ-UMP-001"]

[[items]]
name = "Packet"
kind = "class"
description = "One UMP packet: its message type, group and words."
satisfies = ["REQ-UMP-001"]

[[items]]
name = "Packet.parse"
kind = "fn"
description = "Reads a packet from its words; raises PacketError on a bad length."
satisfies = ["REQ-UMP-001"]
```

It lives in `src/<package>/packet.py` (`[paths].source` is `src/<package>`). `items[]` is the contract surface:
`specdev-analyzer items diff` compares it with the public names in the code, and `/spec-sync-items` reconciles them.
A module with no code yet carries `planned = true`.

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

`check` adds the source rule, stale `planned` flags, imports against `depends_on` (an import under
`if TYPE_CHECKING:` is not a dependency), requirement ↔ test (pytest discovery), and the toolchain rules.

## Gates and CI

`specs/specdev/profile.yaml` declares the gates per tool; the analyzer renders the declared tool's gates into
`scripts/specdev-gates.sh`, which runs `specdev-analyzer check` first and then each gate (uv: `sync`, `tests`,
`types`, `lint`). CI runs the script. Bootstrap writes `.github/workflows/specdev.yml` once (yours afterwards); it
reads the repository variable `SPECDEV_ANALYZER_REPO` and the secret `SPECDEV_ANALYZER_TOKEN` to build the private
analyzer.

**Toolchain CI installs:** uv (which installs the Python the project asks for), and the stable Rust toolchain
(cargo) to build `specdev-analyzer`. The `dev` dependency group holds pytest, pyright and ruff.

CI installs are frozen: commit the lockfile (`uv.lock`); `uv sync --locked` refuses to change it.

## Context files

When a module needs reading that the structural spec cannot carry — a protocol's wire-format quirks, an algorithm
derivation — list it in the module's `context_files = ["docs/protocol-notes.md", ...]`. `specdev-analyzer context
--module` lists those paths for whoever edits the module; a missing file is `CONTEXT-001`. A choice belongs in a
decision, not a context file.

## Extending

Add blocks with `/spec-inline <block>` and bridges with `/spec-inline-bridge <a>+<b>`. Edit
`specs/specdev/domain.toml` to add or change graph checks; once bootstrapped, that file is project-owned.
