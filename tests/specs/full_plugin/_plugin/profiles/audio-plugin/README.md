# audio-plugin profile

A profile for [nih-plug](https://github.com/robbert-vdh/nih-plug)-based
audio plugins targeting VST3, CLAP, and standalone formats from a
single Rust crate.

```
host_interface → plugin_shell → engine → dsp_node
```

Modules declare their layer; dependencies flow downward only (same-layer
allowed, upward forbidden). The plugin crate is a `cdylib`; the standalone format is a `[[bin]]` target.

## When to use this profile

Use `audio-plugin` for projects that:

- Ship as a VST3 / CLAP / standalone audio plugin (no AU in v1).
- Use the nih-plug framework as the host-API abstraction.
- Have a clear separation between plugin shell (parameter declaration,
  editor wiring), engine (DSP coordination, voice management), and
  leaf DSP nodes (oscillators, filters, envelopes).

The recommended layout for non-trivial plugins is a `rust-workspace`
project with two members:

- `crates/engine/` using the `library-api` profile (`no_std`-eligible,
  no plugin concerns) — owns the DSP itself.
- `crates/plugin/` using this `audio-plugin` profile — owns the
  nih-plug shell, parameters, editor.

For small plugins, a single-crate layout works: the plugin and engine
modules live in the same `audio-plugin` crate, separated by `layer`.

Use `library-api` instead if you're shipping a Rust library crate
without plugin concerns. Use `firmware-layered` if your project is
embedded firmware.

## Profile contents

| File | Role |
|---|---|
| `profile.yaml` | analyzer dispatch: language, compile gate, paired files |
| `schemas/module.schema.yaml` | per-module spec shape (layer, parameter bindings) |
| `schemas/parameter_bindings.schema.yaml` | the `[[parameter_bindings]]` row shape |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (plugin/audio/runtime) |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | ledger entry shapes |
| `schemas/enums.schema.yaml` | shared closed vocabularies |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `guidance.md` | implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `spec.toml.template`, `domain.toml.template` | bootstrap seeds |
| `templates/specdev-ci.yml.template` | the CI workflow, written once |

## Architecture-level vocabulary

`specs/architecture.toml` holds `project_decisions` only. The plugin's identity (name, vendor, category, formats),
its buses and its panic strategy are constants in the code and settings in `Cargo.toml`; a spec copy of them had no
check and no reader, so the format does not carry one. `guidance.md` gives their rules.
`Cargo.toml` is the project's: the `cdylib` crate type, the standalone `[[bin]]`, features and dependencies live
there. A standalone `[[bin]]` needs `crate-type = ["cdylib", "lib"]` — with `["cdylib"]` alone the bin cannot link
the crate (the guidance says so).

## Parameter binding model

A `[[parameter_bindings]]` table maps each nih-plug parameter to an engine-side write, so the mapping is stated once
in the spec instead of invented in the code. It is the third leg of the plugin-shell ↔ engine seam: the
`egui+nih-plug` bridge wires parameters → widgets; this wires parameters → engine writes.

The table is **co-resident with `[nih_plug]` on the plugin_shell module** — the shell owns the parameter surface,
and the engine stays framework-agnostic (plain setters/fields). Each row names a source parameter, a target write,
and an optional value transform:

```toml
[[parameter_bindings]]
parameter       = "master_volume"   # resolves to [nih_plug.parameters].id
target_kind     = "method"          # method | field | custom
target          = "ChipEngine::set_master_volume"
target_args     = ["value"]         # `value` = the transformed param value
value_transform = { kind = "db_to_amp" }
```

Six value transforms ship: `identity` (default), `clamp { min, max }`, `scale { factor }`, `enum_to_u8`,
`db_to_amp`, and a `custom { fn }` escape hatch. A parameter may carry several rows. `parameter` must resolve to a
real parameter (reduce time); `parameters_have_bindings` guards the reverse direction. The guidance says how the code
applies the table.

## The audio thread

The guidance carries the audio-thread rules: no allocation, locks or logging in the steady state, and an unfinished
audio-thread function writes silence and logs once instead of panicking — a panic there kills the user's DAW.

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
- `parameters_have_bindings` — every nih-plug parameter has a `[[parameter_bindings]]` row (warning)
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

**Toolchain CI installs:** the stable Rust toolchain (cargo), which also builds `specdev-analyzer`, and on Linux the system libraries nih-plug builds against (ALSA, JACK, OpenGL, X11/XCB, xkbcommon — the workflow installs the list nih-plug's own CI uses).

CI installs are frozen: commit the lockfile, or the CI install refuses to change it.

## Context files

Audio plugins often need reading the structural spec cannot carry — DSP block diagrams, FM-operator routing,
anti-alias filter notes. List them in a module's `context_files = ["docs/dsp-notes.md", ...]`; `specdev-analyzer
context --module` lists them for whoever edits the module, and a missing file is `CONTEXT-001`.

## Extending

Add blocks with `/spec-inline <block>`. v1 ships `nih-plug`
(`[nih_plug]` table on the plugin shell module) and `egui` (`[egui]`
table for editor topology). Bridges with `/spec-inline-bridge <a>+<b>`;
v1 ships `egui+nih-plug` (parameter → widget bindings, plus the egui
host binding).

> **An egui editor needs the bridge.** The `egui` block is host-neutral
> — `nih_plug_egui` belongs to the `egui+nih-plug` bridge, which is this
> stack's egui host. Inline the
> bridge whenever the plugin ships an editor, even before any
> `parameter_widgets` entries exist.

> **Bridge co-residency rule.** `egui+nih-plug` needs both block
> tables (`[nih_plug]` and `[egui]`) AND its bindings table on the
> SAME module (typically the `plugin_shell` module). A binding that
> reaches another module surfaces as a `REDUCE_UNRESOLVED` finding with
> a `hint:` line from the bridge's contract. Each bridge's README
> documents whether co-residency applies.

To customize the layer ordering or add additional graph checks, edit
the project's `specs/specdev/domain.toml` directly — once
bootstrapped, that file is project-owned.

## References

- nih-plug: https://github.com/robbert-vdh/nih-plug
- VST3 SDK: https://steinbergmedia.github.io/vst3_dev_portal/
- CLAP: https://github.com/free-audio/clap
