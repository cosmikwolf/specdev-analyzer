# dart-package profile

A profile for pure Dart packages published to pub.dev whose load-bearing concern is the public API surface.

## When to use this profile

Use `dart-package` for projects that:

- Ship as a Dart package (`lib/<package>.dart` exporting `lib/src/`), possibly published to pub.dev.
- Have a public API worth stating as a contract.
- Want analysis, formatting and tests to run as gates, the way pub.dev scores them.

A Flutter application uses `flutter-app`. A Flutter package (one that depends on the Flutter SDK and needs
`flutter test`) is not covered yet.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, the four gates, paired files |
| `schemas/module.schema.yaml` | per-module spec shape (optional `items[]`; no visibility field — the `_` prefix is the visibility) |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (project decisions) |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template` | bootstrap seeds |
| `templates/analysis_options.yaml.template` | written once: the recommended lints plus the strict language modes |
| `templates/gitignore.template`, `templates/specdev-ci.yml.template` | written once: ignores (`.dart_tool/`, `pubspec.lock`), and the CI workflow |

`pubspec.yaml` is the project's: dependencies and the SDK constraint live there, not in the spec.

## The gates

Dart has one toolchain, so the profile has one gate list: no `toolchains:`, and no `toolchain` in `spec.toml`. In order: `get` (`dart pub get`), `analyze` (`dart analyze --fatal-infos`), `format` (`dart format
--output=none --set-exit-if-changed .`), `test` (`dart test`). pub.dev's scoring tool (pana) runs the analyzer with
the package's options and `dart format`; any lint or unformatted file costs points in "Pass static analysis", so
these gates hold the package to the same bar. `dart analyze` exits 2 when it finds issues — a real finding, not a
missing tool.

## Module shape

```toml
name = "packet"
description = "Builds packets from words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "parse"
kind = "function"
description = "Builds a packet from its words; throws PacketException on a bad length."
satisfies = ["REQ-PKT-001"]
```

It lives in `lib/src/packet.dart`. `items[]` is the contract surface: `specdev-analyzer items diff` compares it with
the public names in the code, and `/spec-sync-items` reconciles them. A module with no code yet carries
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

`check` adds the source rule, stale `planned` flags, imports against `depends_on`, and requirement ↔
test (package:test discovery).

## Gates and CI

`specs/specdev/profile.yaml` declares the gates; the analyzer renders them into
`scripts/specdev-gates.sh`, which runs `specdev-analyzer check` first and then each gate (`get`, `analyze`,
`format`, `test`). CI runs the script. Bootstrap writes `.github/workflows/specdev.yml` once (yours afterwards); it
reads the repository variable `SPECDEV_ANALYZER_REPO` and the secret `SPECDEV_ANALYZER_TOKEN` to build the private
analyzer.

**Toolchain CI installs:** the Dart SDK (`dart-lang/setup-dart`), and the stable Rust toolchain (cargo) to build
`specdev-analyzer`. `lints` and `test` are dev dependencies.

A regular package does not commit `pubspec.lock` (dart.dev, "What not to commit"), so CI resolves the newest
compatible versions on every run: a new dependency release can break CI, which is the point for a library.

## Context files

When a module needs reading that the structural spec cannot carry — a protocol's wire-format quirks, an algorithm
derivation — list it in the module's `context_files = ["docs/protocol-notes.md", ...]`. `specdev-analyzer context
--module` lists those paths for whoever edits the module; a missing file is `CONTEXT-001`. A choice belongs in a
decision, not a context file.

## Extending

Add blocks with `/spec-inline <block>` and bridges with `/spec-inline-bridge <a>+<b>`. Edit
`specs/specdev/domain.toml` to add or change graph checks; once bootstrapped, that file is project-owned.
