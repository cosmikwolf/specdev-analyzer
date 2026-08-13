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
  `nih_export_*!` macro selection and per-format constant emission
  (`VST3_CATEGORIES`, `CLAP_FEATURES`).
- `[audio]` — main bus channel layout, sample-rate bounds, latency,
  optional aux input/output sidechains.
- `[runtime]` — panic strategy (`abort` is required for VST3 cdylib),
  realtime-audit informational flag.

Optional `[[features]]` declares Cargo features (parity with
`library-api`); modules reference these via `feature_gates[]`.

## Triad model

Requirements never travel alone. Three sibling artifacts under
`specs/` carry the project's intent, choices, and unknowns:

| File                  | Role                                                            |
|-----------------------|-----------------------------------------------------------------|
| `requirements.toml`   | the WHAT — intent and constraints, traced to sources            |
| `decisions.toml`      | the HOW & WHY — implementation choices pinned over alternatives |
| `open_questions.toml` | the KNOWN UNKNOWNS — things the project hasn't answered yet     |

A decision (`DEC-001`, "Voice allocation steals the quietest
releasing voice") carries a `status` of `accepted` or `superseded`.
Supersession keeps both bodies as a reasoning trail: the successor
lists `supersedes = ["DEC-003"]`, the old entry flips to `superseded`,
and codegen prompt walks follow only `accepted` decisions. A decision
can also resolve open questions via `closes_questions[]` /
`defaults_for[]` — author ONE direction per cross-reference
(convention: prefer the question-side `closed_by` / `default_via`
pointers; see the comment in `decisions.toml.template`).

Typed references wire decisions into the rest of the spec:

- `requirement.decided_by[]` — pins the *how* of a requirement
- `module.guided_by[]` — informs a module without pinning a requirement
- `architecture.project_decisions[]` — shapes the project as a whole

An open question (`OQ-001`, "PolyBLEP or 4x oversampling for
oscillator anti-aliasing?") lists the requirement ids it `affects` and
moves through three statuses:

- `open` — no committed answer. **Hard-blocks codegen**: `gen` aborts
  for any module whose `satisfies[]` reaches this question through its
  `affects[]` — an unanswered anti-aliasing question stops codegen for
  the oscillator `dsp_node`, not for the editor. No CLI flag bypasses
  the gate.
- `default_committed` — a decision committed a working default
  (`default_via`), with a one-line `verify` falsification plan (say,
  "5 ms parameter smoothing; sweep the filter cutoff at full
  modulation depth and listen for zipper noise"). Codegen proceeds;
  the placeholder and `verify` text are inlined into the prompt for
  affected modules.
- `closed` — a decision resolved it (`closed_by`). Codegen proceeds;
  the prompt inlines the resolved answer. The entry persists as audit
  trail.

The in-spec escape from the gate is downgrading a question to
`default_committed` — committing a provisional default through a
decision. Use `/spec-question` for that flow and `/spec-decide` to
record decisions.

Decisions aren't passive documentation: along the module → requirement
→ `decided_by` walk, each accepted decision's rationale text is
inlined into the codegen prompt, so a recorded choice (say,
quietest-voice stealing over oldest-note) directly shapes the
generated engine code.

Validation backs all of this. `validate` checks that supersession
chains are acyclic and consistent (every superseded entry is named by
exactly one accepted successor), that every cross-reference resolves,
and it warns about gate-blocking open questions before `gen` refuses.

## Graph checks shipped

- `unique_qualified_names` — every entity has a unique qualified name (collisions across kinds are surfaced).
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
- `parameters_have_bindings` — orphan-check: every nih-plug parameter
  must be wired to at least one engine write via `[[parameter_bindings]]`
  (warning). See "Parameter binding model" below.

## Parameter binding model

A `[[parameter_bindings]]` table maps each nih-plug parameter to an
engine-side write, so the analyzer emits a deterministic
`apply_param_changes` dispatcher instead of leaving the 100+-parameter
mapping for codegen to invent (the drift the second zfm dogfood
surfaced). It's the third leg of the plugin-shell ↔ engine bridge: the
`egui+nih-plug` bridge wires parameters → widgets; this wires
parameters → engine writes.

The table is **co-resident with `[nih_plug]` on the plugin_shell
module** — the shell owns the parameter surface, and the engine stays
framework-agnostic (it exposes plain setters/fields and never hears of
nih-plug). Each row names a source parameter, a target write, and an
optional value transform:

```toml
[[parameter_bindings]]
parameter       = "master_volume"   # resolves to [nih_plug.parameters].id
target_kind     = "method"          # method | field | custom
target          = "ChipEngine::set_master_volume"
target_args     = ["value"]         # `value` = the transformed param value
value_transform = { kind = "db_to_amp" }
```

Six value transforms ship: `identity` (default), `clamp { min, max }`,
`scale { factor }`, `enum_to_u8`, `db_to_amp`, and a `custom { fn }`
escape hatch. A single parameter may carry **multiple rows**
(multi-target); codegen emits all matching arms in declaration order.
Reference resolution (`parameter` → a real parameter) is enforced at
reduce time; the orphan check guards the reverse direction. See
`codegen/rust/module.conventions.md` § "Parameter binding model" for the
full emit rules.

## Cargo.toml emission

Cargo.toml is **emitted by the analyzer**, not hand-edited:

| `[plugin].formats`            | Plugin Cargo.toml shape                                                |
|-------------------------------|------------------------------------------------------------------------|
| any subset without standalone | `[lib] crate-type = ["cdylib"]`                                        |
| any subset with standalone    | `[lib] crate-type = ["cdylib"]` + `[[bin]] name = "<plugin>-standalone"` |

The standalone bin path is always `src/bin/standalone.rs`; codegen
emits this file when standalone is in the format set. The bin links
the same lib so the `Plugin` impl is reused.

## Audio-thread method bodies

Audio-thread methods on the plugin shell — `process`, `process_buffers`,
and helpers invoked from inside them — emit a **silence-and-log** body
in first-pass `gen` output: write zeros into the output buffer, fire
one diagnostic `nih_log!` on first invocation, and return the normal
process status. Non-audio-thread code (`initialize`, `reset`, editor /
state plumbing, hand-authored helpers that don't run on the audio
thread) emits `unimplemented!()` so the panic surfaces the missing
implementation loudly during authoring.

The split is load-bearing: `unimplemented!()` on the audio thread
aborts the host (the user's DAW). Silence-and-log on the audio thread
keeps the host alive long enough to verify the bundle loads and the
stub surface is wired up correctly. See
`codegen/rust/module.conventions.md` § "Audio-thread method bodies"
for the full emit template (with `AtomicBool` once-only logging) and
the `runtime.realtime_audit` interaction.

## Out-of-band context (escape hatch)

Audio plugins often carry algorithmic prose that doesn't fit the
structural spec — DSP block diagrams, FM-operator routing, anti-alias
filter design notes, MIDI controller mappings. Per-module
`context_files = ["docs/dsp-notes.md", ...]` inlines those prose paths
into the assembled codegen prompt. Paths are project-relative; missing
files raise `CONTEXT-001` at validate time. Use this for audio-thread
math and protocol prose; structural facts (parameters, ports, panels,
bindings) belong in the schema-driven tables.

## Extending

Add blocks via `specdev inline <block>`. v1 ships `nih-plug`
(`[nih_plug]` table on the plugin shell module) and `egui` (`[egui]`
table for editor topology). Bridges via `specdev inline-bridge <a>+<b>`;
v1 ships `egui+nih-plug` (parameter → widget bindings, plus the egui
host binding).

> **An egui editor needs the bridge.** The `egui` block is host-neutral
> and contributes no crates — `nih_plug_egui` comes from the
> `egui+nih-plug` bridge, which is this stack's egui host. Inline the
> bridge whenever the plugin ships an editor, even before any
> `parameter_widgets` entries exist.

> **Bridge co-residency rule (v1).** When you inline a bridge whose
> two endpoint blocks both contribute tables on a module
> (e.g. `egui+nih-plug` requires both `[nih_plug]` and `[egui]`), all
> three — both block tables AND the bindings table — must live on the
> SAME module (typically the `plugin_shell` module). v1 does not
> support cross-module bindings; the analyzer surfaces this as a
> `REDUCE_UNRESOLVED` finding with a `hint:` line pointing at the
> bridge's README. Each bridge's README documents whether co-residency
> applies.

To customize the layer ordering or add additional graph checks, edit
the project's `specs/specdev/domain.toml` directly — once
bootstrapped, that file is project-owned.

## References

- nih-plug: https://github.com/robbert-vdh/nih-plug
- VST3 SDK: https://steinbergmedia.github.io/vst3_dev_portal/
- CLAP: https://github.com/free-audio/clap
