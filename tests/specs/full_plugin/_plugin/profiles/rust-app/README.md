# rust-app profile

A profile for standalone Rust applications — a desktop app, a CLI, a
service — built as a **testable core behind `src/lib.rs` plus a thin
`src/main.rs` entry point**, in one crate.

## When to use this profile

Use `rust-app` for projects that:

- Build an executable that is not firmware and not hosted by a plugin
  host — the app owns its own `main`.
- Want their logic testable without launching the binary. The `[lib]`
  target is where the specs live; `cargo test` exercises it directly.
- Have a layering worth stating and enforcing (entry / adapter / core /
  support), rather than a flat set of modules.

Don't use this profile if:

- The crate is consumed by *other* crates and its public API is the
  product — that's `library-api`, where the API surface, re-export
  discipline and semver are the load-bearing concerns.
- The target is embedded firmware with a hardware surface (pins,
  peripherals, ISRs) — that's `firmware-layered`.
- A host supplies the window and the event loop — that's
  `audio-plugin`.
- The project is several crates. Use `rust-workspace` at the root and
  give each member its own profile; `rust-app` is a fine choice for a
  binary member.

## Why one crate with two targets (RUST-APP-LIB-PLUS-BIN)

The obvious alternative — a workspace with a `core` library member and a
`gui` binary member — is a heavier default than most applications need,
and it is easy to adopt later. One crate with `src/lib.rs` +
`src/main.rs` gets the property that actually matters (logic testable
without starting the process) at zero structural cost:

1. **`cargo test` already does the right thing.** Tests link the lib
   target; nothing has to start, bind a port, or open a window.
2. **The split costs one line.** `main.rs` calls into the lib and
   returns. There is no dependency edge to declare, no second spec tree,
   no version to keep synced between two crates.
3. **Splitting later is mechanical.** When a second consumer of the core
   appears, `/spec-add-member` adopts this crate into a workspace with
   its requirement and decision ids intact. Splitting *before* that
   consumer exists buys nothing and costs a boundary you must maintain.

The practical rule this profile states: **`src/main.rs` makes one call into
the lib and nothing else.** Startup logic that deserves a test goes in an
`entry`-layer module under the lib, and `main` calls it.

## Layers

Modules declare a `layer`; the `layer_dependency` graph check enforces
downward-only dependencies in the spec (same-layer is allowed, upward is an error), and `check` holds the code's
imports to the declared `depends_on`.

| layer | holds | rule |
| --- | --- | --- |
| `entry` | startup wiring, config and argument parsing, building the object graph | nothing here should be worth unit-testing on its own |
| `adapter` | everything touching the outside world: network, filesystem, database, GUI, clock, other processes | a real implementation and a test double should both be plausible |
| `core` | the application's own types and rules | no I/O, no framework imports — testable with no runtime |
| `support` | error types, logging setup, small shared helpers | keep it small; a growing `support` usually means something belongs in `core` |

`core` importing nothing is the load-bearing constraint — it's what makes
the application testable at all. When `core` needs the outside world,
declare a trait in `core`, implement it in `adapter`, and wire the two in
`entry`.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, compile gate, paired files |
| `schemas/module.schema.yaml` | per-module spec shape (layer, visibility, optional `items[]`) |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (project decisions) |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies (`Layer`, `Visibility`) |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template` | bootstrap seeds |
| `templates/specdev-ci.yml.template` | the CI workflow, written once |

`Cargo.toml` is the project's: the `[lib]` and `[[bin]]` targets, features and dependencies live there.

## Module shape

A `rust-app` Module is a unit of the application, placed on the layer axis:

```toml
name = "PeerSession"
layer = "core"
visibility = "pub"
description = "Tracks the state of one connected peer."
depends_on = ["WireFormat"]
satisfies = ["REQ-NET-002"]
planned = true
```

It lives in `src/peer_session.rs`, declared from `src/lib.rs` as `pub mod peer_session;`. `planned = true` says the
code does not exist yet; remove it when it lands.

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
- `layer_dependency` — no upward dependency in the layer ordering
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

## Composing with blocks

`rust-app` deliberately has no framework surface, which makes it the neutral host for blocks. A desktop GUI app
inlines the blocks for its stack (window/shell, UI topology, transport); each contributes its own namespaced table on
the modules that need it, and its own implementation guidance. The profile itself stays out of the way.
