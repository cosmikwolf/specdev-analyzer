# flutter-app profile

Spec-driven development for Flutter applications: single-codebase apps
targeting any subset of android / ios / web / macos / windows / linux,
with modules classified by architectural role and fenced by platform.

## When to use this profile

Use `flutter-app` when the project is a Flutter **application** — it
ships to devices/browsers, is never published to pub.dev, and its
load-bearing concerns are UI structure, platform coverage, and pub
dependencies.

Not this profile:

- A publishable Dart/Flutter *package* — no profile covers that shape
  yet.
- A Rust library consumed by the app over FFI — give it its own spec
  root under `library-api` (or `rust-workspace`); the coupling between
  the two roots is build-layer configuration, not a spec relation.

## Profile contents

| File | Purpose |
|------|---------|
| `profile.yaml` | Analyzer dispatch manifest (`language: dart`, pubspec shape `app`, environment defaults) |
| `schemas/module.schema.yaml` | Module shape: `role`, `platforms`, `channels`, deps, items |
| `schemas/architecture.schema.yaml` | Platform matrix, app version, `flutter:` block, environment, project-wide deps |
| `schemas/enums.schema.yaml` | Role / PlatformName / identifier vocabularies |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | The triad (same shapes as the other profiles) |
| `reductions/project.yaml` | Entity/relation projection (Module, Platform, Channel, triad kinds) |
| `domain.toml.template` | Graph checks incl. `role_dependency` + `channel_names_unique` |
| `codegen/dart/module.conventions.md` | How a module spec maps to Dart source |
| `*.toml.template` | Bootstrap seeds for spec.toml / architecture / requirements / decisions / open questions |

## Module shape

Every module carries a `role` — its architectural tier:

```
app > screen > widget > service > platform_channel > model
```

`app` is the composition root (shell, routing, DI); `model` is pure
Dart. The `role_dependency` graph check warns on upward dependencies
(a service importing a widget, anything importing `app`). Same-role and
downward dependencies are allowed.

Two more Dart-shaped fields:

- **`platforms`** — restricts a module to a subset of the project's
  platform matrix (`platforms` in architecture.toml). Omitted = all
  targets. Entries outside the matrix fail validation. Codegen states
  the fence in the module's doc header and guards web-excluded usage.
- **`channels`** — for `role = "platform_channel"` modules only: the
  named platform channels the module owns (`{ name, kind }`, kind ∈
  method / event / basic_message). Names are the cross-language
  contract with the native side and must be globally unique
  (`channel_names_unique`).

Dependencies are pub packages (`external_deps` PubReference entries:
pub.dev / hosted / git / path / sdk sources; `test_only` routes to
dev_dependencies). Emit pubspec.yaml from the spec with:

```sh
specdev-analyzer manifest --write
```

(`/spec-gen` runs this as its manifest step.) pubspec is output, never
hand-maintained.

A minimal complete module file (`specs/modules/CounterService.toml` —
PascalCase `name`, filename matching it; prefer `/spec-design` for
guided authoring):

```toml
name = "CounterService"
role = "service"
description = "Holds the counter value and notifies listeners on change."
satisfies = ["REQ-SVC-001"]

[[items]]
name = "CounterService"
kind = "class"
description = "Listenable counter state holder."
satisfies = ["REQ-SVC-001"]
```

Validate emits an advisory `CONTEXT-100` warning for modules without
`context_files` — it is safe to ignore for small modules; discharge it
by pointing the module at project prose (`context_files =
["docs/counter.md"]`) when out-of-band context genuinely exists.

## Source layout

Module names are PascalCase type-style identifiers (`CounterService`);
each emits to `lib/src/<snake>.dart` (`CounterService` →
`lib/src/counter_service.dart`; `[paths] source = "lib/src"` in
spec.toml). The `app`-role module also owns the `lib/main.dart`
delegation stub. Platform directories (`android/`, `ios/`, `web/`, …)
are owned by Flutter tooling and are never generated or gen-recorded —
see the conventions doc's "Generated-file ownership".

## Triad model

Identical to the other profiles: requirements (+ sources), decisions
(the *how & why*), open questions (the *known unknowns*), with the
codegen gate refusing generation for modules reachable through an open
question. See each template's header comments for authoring guidance.

## Graph checks shipped

- `unique_qualified_names`, `modules_acyclic`, `requirement_ids_unique`
- `role_dependency` (warning) — role-ordering over `depends_on`
- `channel_names_unique` — global channel-name uniqueness
- `requirements_have_satisfiers`, `test_requirements_have_verifiers`
- Triad consistency: supersession chain shape, OQ status consistency,
  `codegen_unblocked`

Module `platforms[]` membership in the project matrix is enforced at
reduce time (an out-of-matrix entry is an unresolved reference), not by
a graph check.

## Deferred (named, not silently missing)

- **Cross-module platform narrowing** — a check that a module's
  dependencies support every platform the module claims. Today the
  fence is stated per-module and honored by the codegen guard idiom.
- **Custom font declarations** in the `flutter:` block.
- **Build-time configuration** (`--dart-define` flags, flavors).
- **A publishable-package profile** and a Dart workspace root shape.
- **Routing/state-management typed fields** — navigation and state
  architecture are carried by decisions + module prose in v1; blocks
  can layer framework specifics.

## Extending

Blocks attach to modules through the `bindings` aggregator slot and may
contribute their own schema tables; inline them with `/spec-inline`.
Framework-specific codegen prose layers via each block's
`codegen/dart/README.md` on top of the conventions doc here.
