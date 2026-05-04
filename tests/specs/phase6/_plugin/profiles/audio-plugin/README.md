# audio-plugin profile

A profile for [nih-plug](https://github.com/robbert-vdh/nih-plug)-based
audio plugins targeting VST3, CLAP, and standalone formats from a
single Rust crate.

```
host_interface → plugin_shell → engine → dsp_node
```

Modules declare their layer; dependencies flow downward only (same-layer
allowed, upward forbidden). The plugin crate is a `cdylib` with an
optional `[[bin]]` standalone target.

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

| File                                       | Role                                                   |
|--------------------------------------------|--------------------------------------------------------|
| `schemas/module.schema.yaml`               | per-module spec shape                                  |
| `schemas/architecture.schema.yaml`         | `specs/architecture.toml` shape (plugin/audio/runtime) |
| `schemas/requirements.schema.yaml`         | `specs/requirements.toml` shape (sources + reqs)       |
| `schemas/enums.schema.yaml`                | shared closed vocabularies                             |
| `reductions/project.yaml`                  | TOML → universal entity/relation graph                 |
| `codegen/rust/module.conventions.md`       | Rust emission conventions                              |
| `spec.toml.template`                       | bootstrap-time `spec.toml` skeleton                    |
| `domain.toml.template`                     | bootstrap-time `domain.toml` skeleton + graph checks   |
| `architecture.toml.template`               | bootstrap-time empty architecture file with examples   |
| `requirements.toml.template`               | bootstrap-time empty requirements file with examples   |

## Architecture-level vocabulary

Three required tables in `specs/architecture.toml`:

- `[plugin]` — vendor / display / formats / category. Drives
  `nih_export_*!` macro selection (D45) and per-format constant
  emission (`VST3_CATEGORIES`, `CLAP_FEATURES` per D46).
- `[audio]` — main bus channel layout, sample-rate bounds, latency,
  optional aux input/output sidechains.
- `[runtime]` — panic strategy (`abort` is required for VST3 cdylib),
  realtime-audit informational flag.

Optional `[[features]]` declares Cargo features (parity with
`library-api`); modules reference these via `feature_gates[]`.

## Graph checks shipped

- `unique_module_names` — every Module has a unique name.
- `modules_acyclic` — module dependency graph has no cycles.
- `layer_dependency` — no upward dependency in the layer ordering
  (`host_interface` → `plugin_shell` → `engine` → `dsp_node`).
- `feature_unique_names` — Cargo feature names are unique.
- `feature_dependencies_resolve` — `[[features]] depends_on` references
  resolve.
- `module_feature_gates_resolve` — `module.feature_gates[]` references
  resolve.
- `requirement_ids_unique` — every Requirement id is unique.
- `requirement_satisfies_resolves` — Requirement.satisfies references
  exist.
- `module_satisfies_resolves` — Module.satisfies references exist.
- `requirements_have_satisfiers` — orphan-check on `nature = "module"`
  requirements.

## Cargo.toml emission (D45)

Cargo.toml is **emitted by the analyzer**, not hand-edited:

| `[plugin].formats`            | Plugin Cargo.toml shape                                                |
|-------------------------------|------------------------------------------------------------------------|
| any subset without standalone | `[lib] crate-type = ["cdylib"]`                                        |
| any subset with standalone    | `[lib] crate-type = ["cdylib"]` + `[[bin]] name = "<plugin>-standalone"` |

The standalone bin path is always `src/bin/standalone.rs`; codegen
emits this file when standalone is in the format set. The bin links
the same lib so the `Plugin` impl is reused.

## Skeleton mode (D49)

Skeleton mode (`gen --skeleton`) emits `unimplemented!()` stubs.
Because panicking on the audio thread would abort-crash the host, the
plugin crate is **excluded** from skeleton mode in v1. The engine
crate (when it uses `library-api`) gets skeleton mode normally;
silence-and-log stubs for the plugin crate are a bounded follow-up.

## Extending

Add blocks via `specdev inline <block>` once they ship — Phase 15-C
ships `nih-plug` (`[nih_plug]` table on the plugin shell module) and
Phase 15-D ships `egui` (`[egui]` table for editor topology). Bridges
via `specdev inline-bridge <a>+<b>` — Phase 15-E ships
`egui+nih-plug` (parameter → widget bindings).

To customize the layer ordering or add additional graph checks, edit
the project's `specs/specdev/domain.toml` directly — once
bootstrapped, that file is project-owned.

## References

- Phase 15-A spike: `specdev-next/000-audio-plugin-spike-2026-05-03.md`
- nih-plug: https://github.com/robbert-vdh/nih-plug
- VST3 SDK: https://steinbergmedia.github.io/vst3_dev_portal/
- CLAP: https://github.com/free-audio/clap
