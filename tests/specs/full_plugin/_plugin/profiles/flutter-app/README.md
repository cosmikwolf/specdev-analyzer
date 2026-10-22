# flutter-app profile

Spec-driven development for Flutter applications: single-codebase apps
targeting any subset of android / ios / web / macos / windows / linux,
with modules classified by architectural role and fenced by platform.

## When to use this profile

Use `flutter-app` when the project is a Flutter **application** — it
ships to devices/browsers, is never published to pub.dev, and its
load-bearing concerns are UI structure and platform coverage.

Not this profile:

- A publishable Dart/Flutter *package* — no profile covers that shape
  yet.
- A Rust library consumed by the app over FFI — give it its own spec
  root under `library-api` (or `rust-workspace`); the coupling between
  the two roots is build-layer configuration, not a spec relation.

## Profile contents

| File | Purpose |
|------|---------|
| `profile.yaml` | Analyzer dispatch manifest (`language: dart`, compile gate, paired files) |
| `schemas/module.schema.yaml` | Module shape: `role`, `platforms`, `channels`, deps, optional items |
| `schemas/architecture.schema.yaml` | The platform matrix and project decisions |
| `schemas/enums.schema.yaml` | Role / PlatformName / identifier vocabularies |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | Ledger entry shapes |
| `reductions/project.yaml` | Entity/relation projection (Module, Platform, Channel, ledger kinds) |
| `domain.toml.template` | Graph checks incl. `role_dependency` + `channel_names_unique` |
| `guidance.md` | Implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `architecture.toml.template` | Bootstrap seeds |
| `templates/specdev-ci.yml.template` | The CI workflow, written once |

## Module shape

Every module carries a `role` — its architectural tier:

```
app > screen > widget > service > platform_channel > model
```

`app` is the composition root (shell, routing, DI); `model` is pure Dart. The `role_dependency` graph check warns on
upward dependencies (a service importing a widget, anything importing `app`). Same-role and downward dependencies are
allowed; `check` holds the code's imports to the declared `depends_on`.

Two more Dart-shaped fields:

- **`platforms`** — restricts a module to a subset of the project's platform matrix (`platforms` in
  architecture.toml). Omitted = all targets. Entries outside the matrix fail validation. The guidance says how the
  code states and honors the fence.
- **`channels`** — for `role = "platform_channel"` modules only: the named platform channels the module owns
  (`{ name, kind }`, kind ∈ method / event / basic_message). Names are the cross-language contract with the native
  side and must be globally unique (`channel_names_unique`).

`pubspec.yaml` is the project's: pub dependencies live there, not in the spec.

A minimal complete module file (`specs/modules/CounterService.toml` — PascalCase `name`, filename matching it; prefer
`/spec-design` for guided authoring):

```toml
name = "CounterService"
role = "service"
description = "Holds the counter value and notifies listeners on change."
satisfies = ["REQ-SVC-001"]
planned = true
```

## Source layout

Each module lives in `lib/src/<snake>.dart` (`CounterService` → `lib/src/counter_service.dart`; `[paths] source =
"lib/src"` in spec.toml). `lib/main.dart` delegates to the `app`-role module. Platform directories (`android/`,
`ios/`, `web/`, …) are owned by Flutter tooling: run `flutter create . --platforms <matrix>` once by hand before the
first real build, and delete the scaffold's `test/widget_test.dart` afterwards — it references template symbols the
app does not have.

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
- `role_dependency` — no upward dependency in the role ordering (warning)
- `channel_names_unique` — platform channel names are unique
- `requirement_ids_unique` — requirement ids are unique
- `decision_ids_unique` — decision ids are unique
- `requirements_have_satisfiers` — every requirement is satisfied by a module (error; a `planned` requirement is exempt)
- `decision_supersedes_acyclic` — supersession chains have no cycle
- `decision_supersedes_chain_consistency` — every superseded decision is named by exactly one successor, and every decision named in `supersedes` is `superseded`
- `decision_reference_superseded` (warning) — a module's `guided_by` or a requirement's `decided_by` names a superseded decision
- `open_question_closed_by_resolves` — `closed_by` names an existing decision
- `open_question_status_consistency` — `closed_by` / `default_via` name accepted decisions
- `implementation_unblocked` — the open-question gate, run by `check`: an `open` question reaches no module that has code

`check` adds the source rule, stale `planned` flags, imports against `depends_on`, and requirement ↔ test.

Module `platforms[]` membership in the project matrix is enforced at reduce time (an out-of-matrix entry is an
unresolved reference), not by a graph check.

## Gates and CI

`specs/specdev/profile.yaml` declares the compile gates; the analyzer renders them into `scripts/specdev-gates.sh`,
which runs `specdev-analyzer check` first and then each gate. CI runs the script. Bootstrap writes
`.github/workflows/specdev.yml` once (yours afterwards); it reads the repository variable `SPECDEV_ANALYZER_REPO` and
the secret `SPECDEV_ANALYZER_TOKEN` to build the private analyzer.

**Toolchain CI installs:** the Flutter SDK (stable channel; `flutter pub get`, `flutter analyze`), and the stable Rust toolchain to build `specdev-analyzer`.

CI installs are frozen: commit the lockfile, or the CI install refuses to change it.

The gate `flutter-analyze` runs `flutter pub get`, then `flutter analyze --no-fatal-infos --no-fatal-warnings`:
errors fail the gate; infos and warnings do not, the same bar as `cargo check`. Platform scaffolding is not the
gate's finding: both steps pass with no platform directories.

## Deferred (named, not silently missing)

- **Cross-module platform narrowing** — a check that a module's
  dependencies support every platform the module claims. Today the
  fence is stated per-module and honored by the guard idiom in the guidance.
- **Custom font declarations** in the `flutter:` block.
- **Build-time configuration** (`--dart-define` flags, flavors).
- **A publishable-package profile** and a Dart workspace root shape.
- **Routing/state-management typed fields** — navigation and state
  architecture are carried by decisions + module prose in v1; blocks
  can layer framework specifics.

## Extending

Blocks contribute their own **top-level** namespaced tables to modules (composed through the module schema's `allOf`
aggregation); the `bindings` slot is reserved for *bridge* contributions. Inline blocks with `/spec-inline`. Each
block's guidance adds its framework rules on top of this profile's.
