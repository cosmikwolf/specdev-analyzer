# python-ffi profile

A profile for an importable Python package built around a compiled Rust extension: PyO3 bindings, built into the
wheel by maturin. The Python package is this specdev project; the Rust crate is a second one.

## When to use this profile

Use `python-ffi` for a Python package whose core is a Rust library:

- The work is in Rust; Python gets a typed, idiomatic API over it.
- maturin builds the wheel (`[build-system] build-backend = "maturin"`).
- The native module is described by a hand-written `.pyi` stub, and the stub is held to the built module.

A pure-Python package uses `python-library`.

## Two specdev projects

| Project | Profile | Holds |
|---|---|---|
| the Python package (this profile), at the `pyproject.toml` root | `python-ffi` | the Python modules, the native module's stub, the tests |
| the Rust crate, in the directory `[tool.maturin] manifest-path` names (`rust/` by default, or a member of a Rust workspace) | `library-api` with the `pyo3` block inlined | the `#[pymodule]`, the `[pyo3]` table of exported names |

Each runs its own `check` and gates. The `pyo3` block's code checks hold the Rust exports to its `[pyo3]` table; this
profile's `stubs` gate holds the stub to the built module; `items diff` holds the spec to the stub.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, `toolchains` (gates and lockfiles per tool), paired files |
| `schemas/module.schema.yaml` | per-module spec shape (optional `items[]`); the native module's spec names its stub in `source_file` |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml`: `[native] module` (required — the `stubs` gate reads it) and project decisions |
| `architecture.toml.template` | seeds `[native] module = "<project>._native"` |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template` | bootstrap seeds |
| `templates/specdev-ci.yml.template` | the CI workflow, written once (for uv) |

## The tool and the gates

The project declares its tool in `spec.toml`: `[project] toolchain = "uv"` (the default), `"poetry"` or `"pip"`.
Each tool's gates, in order:

| Gate | uv | What it proves |
|---|---|---|
| `sync` (pip: `build`) | `uv sync --locked` | the extension builds (maturin, through the build backend) and installs |
| `stubs` | `python -m mypy.stubtest <[native] module>` | the `.pyi` stub matches the built module: names, `__all__`, `@final` |
| `tests` | `python -m pytest` | the requirements' tests pass against the built extension |
| `types` | `python -m pyright` | the package type-checks against the stub |
| `lint` | `python -m ruff check` | lint |

With uv, list the package in `[tool.uv] reinstall-package`: uv does not see a change in the Rust crate and otherwise
keeps the old extension (measured: the stub check then passes against stale code). With `pip`, start the gate
script inside an activated virtual environment that has the `dev` group installed.

## Module shape

The native module's spec names its stub, and its items are the native API:

```toml
name = "_native"
description = "The compiled extension, described by its stub."
source_file = "src/my_package/_native.pyi"
satisfies = ["REQ-PKT-001"]

[[items]]
name = "Packet"
kind = "class"
description = "A packet of one to four words."
satisfies = ["REQ-PKT-001"]
```

Python modules (`src/my_package/<name>.py`) are specced as in `python-library`.

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
`scripts/specdev-gates.sh`, which runs `specdev-analyzer check` first and then each gate (uv: `sync`, `stubs`,
`tests`, `types`, `lint`). CI runs the script. Bootstrap writes `.github/workflows/specdev.yml` once (yours afterwards); it
reads the repository variable `SPECDEV_ANALYZER_REPO` and the secret `SPECDEV_ANALYZER_TOKEN` to build the private
analyzer.

**Toolchain CI installs:** uv (which installs the Python the project asks for), and the stable Rust toolchain
(cargo), which builds the extension and `specdev-analyzer`. The `dev` dependency group holds pytest, pyright, ruff
and mypy (for stubtest).

CI installs are frozen: commit the lockfile (`uv.lock`); `uv sync --locked` refuses to change it.

## Context files

When a module needs reading that the structural spec cannot carry — a protocol's wire-format quirks, an algorithm
derivation — list it in the module's `context_files = ["docs/protocol-notes.md", ...]`. `specdev-analyzer context
--module` lists those paths for whoever edits the module; a missing file is `CONTEXT-001`. A choice belongs in a
decision, not a context file.

## Extending

Add blocks with `/spec-inline <block>` and bridges with `/spec-inline-bridge <a>+<b>`. Edit
`specs/specdev/domain.toml` to add or change graph checks; once bootstrapped, that file is project-owned.
