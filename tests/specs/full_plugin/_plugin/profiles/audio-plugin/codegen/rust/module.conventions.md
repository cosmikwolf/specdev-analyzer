# audio-plugin: Rust module conventions

These conventions describe how an `audio-plugin` profile module spec
maps to Rust source. They are language-and-architecture-only —
framework specifics (parameter declaration, audio-port wiring, editor
topology) come from the relevant block's `codegen/rust/README.md`,
layered on top.

This profile targets the [nih-plug](https://github.com/robbert-vdh/nih-plug)
audio plugin framework, which builds VST3 / CLAP / standalone artifacts
from a single Rust crate.

## Crate shape

A plugin crate is a `cdylib` (the C-ABI surface VST3 and CLAP both
require) with an optional `[[bin]]` target for the standalone
distribution format. Cargo.toml is **emitted by the analyzer** from the
spec — do not hand-edit it; edit the spec and regenerate.

| `[plugin].formats` | Cargo.toml shape                                                |
|--------------------|-----------------------------------------------------------------|
| no `standalone`    | `[lib] crate-type = ["cdylib"]`                                 |
| with `standalone`  | `[lib] crate-type = ["cdylib"]` + `[[bin]] name = "<plugin>-standalone"` |

The bin path is always `src/bin/standalone.rs`; codegen emits this file
when standalone is in the format set. The bin links the same crate's
`lib`, so the plugin shell's `impl Plugin for X` is reused unchanged.

## File layout

```
src/
├── lib.rs                        # crate root: declares modules, runs nih_export_*!
├── <plugin_shell_module>.rs      # impl Plugin for X, parameter struct, editor wiring
├── <engine_module>.rs            # DSP coordination (or in a sibling `engine` crate)
├── <dsp_node_module>.rs          # leaf DSP units
└── bin/
    └── standalone.rs             # nih_export_standalone!(); only when format set includes standalone
```

For multi-crate workspaces (the recommended layout): the engine lives
in a sibling member crate using the `library-api` profile (`no_std`-
eligible, no plugin concerns); the
plugin crate uses this profile and depends on the engine via Cargo
path/workspace dep. Cross-crate boundary is the engine ↔ plugin_shell
edge — see profile README §"When to use this profile."

The crate root (`src/lib.rs`) declares each top-level module and runs
the format-driven export macros:

```rust
mod my_plugin;
mod synthesis;          // engine module (single-crate layout)
mod oscillator;         // dsp_node module

use my_plugin::MyPlugin;

nih_export_vst3!(MyPlugin);
nih_export_clap!(MyPlugin);
```

Emit one `nih_export_<fmt>!(<PluginShellType>)` line per format in
`[plugin].formats`, in declaration order. `standalone` does NOT use
`nih_export_standalone!` here — that lives in `src/bin/standalone.rs`
(which calls `nih_export_standalone!(my_plugin::MyPlugin)`).

## Layer emission rules

`module.layer` drives where the module lives in the dependency
hierarchy. The four-tier ordering:

| Layer            | Typical content                                          | Allowed deps           |
|------------------|----------------------------------------------------------|------------------------|
| `host_interface` | Reserved — nih-plug-provided types. No user modules.     | —                      |
| `plugin_shell`   | `impl Plugin for X`, parameter struct, editor wiring     | `engine`, `dsp_node`   |
| `engine`         | Voice management, preset state, DSP coordination         | `dsp_node`             |
| `dsp_node`       | Leaf DSP units (oscillators, filters, envelopes)         | (leaf — no deps)       |

The `layer_dependency` graph check rejects upward dependencies. v1
restricts each plugin crate to **one** `plugin_shell` module — multi-shell
per crate is a bounded follow-up. Modules at `host_interface`
are not authored; the value exists in the enum so the layer ordering
has a complete top edge.

Engine modules MAY be `no_std`-compatible — when the engine lives in a
sibling `library-api` crate, that crate carries its own no_std
discipline; the plugin crate (this profile) has no no_std mode (cdylib
links libstd).

## Plugin shell emission

The `plugin_shell` module is the load-bearing one. Its emission
template — populated from `[plugin]` in architecture.toml plus the
`[nih_plug]` block contribution on the same module:

```rust
use nih_plug::prelude::*;
use std::sync::Arc;

pub struct MyPlugin {
    params: Arc<MyPluginParams>,
    // engine + DSP state lives here
}

#[derive(Params)]
pub struct MyPluginParams {
    // parameters from [[nih_plug.parameters]] — block prose drives this
}

impl Plugin for MyPlugin {
    const NAME: &'static str    = "<from [plugin].name>";
    const VENDOR: &'static str  = "<from [plugin].vendor>";
    const URL: &'static str     = "<from [plugin].url, or empty string when omitted>";
    const EMAIL: &'static str   = "<from [plugin].email, or empty string when omitted>";
    const VERSION: &'static str = "<from [plugin].version>";

    const VST3_CATEGORIES: &'static str = /* per-category lookup, see below */;
    const CLAP_FEATURES: &'static [ClapFeature] = /* per-category slice */;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> { self.params.clone() }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Real-time discipline applies here; see "Realtime path" below.
        ProcessStatus::Normal
    }
}
```

`URL` and `EMAIL` are `&'static str` constants; nih-plug requires them
to be set. When `[plugin].url` / `email` is omitted in the spec, emit
the literal `""` empty string (the host displays nothing).

## Per-format constants

`[plugin].category` is a closed enum mapped to per-format constants by
this lookup table. Codegen reads category once and emits all relevant
constants:

| `category`   | `VST3_CATEGORIES`             | `CLAP_FEATURES` slice                                                |
|--------------|-------------------------------|----------------------------------------------------------------------|
| `instrument` | `"Instrument\|Synth"`          | `&[ClapFeature::Instrument, ClapFeature::Synthesizer]`               |
| `fx`         | `"Fx"`                        | `&[ClapFeature::AudioEffect]`                                        |
| `midi-fx`    | `"Fx\|MIDI"`                   | `&[ClapFeature::NoteEffect]`                                         |
| `analyzer`   | `"Fx\|Analyzer"`               | `&[ClapFeature::Analyzer, ClapFeature::AudioEffect]`                 |

Standalone has no category constant — `nih_export_standalone!` consumes
the same `Plugin` impl and shows the plugin under its `NAME` only.

If `formats` does not include `vst3`, omit the `VST3_CATEGORIES` const.
If `formats` does not include `clap`, omit `CLAP_FEATURES`. nih-plug
tolerates absent-but-unused constants; emitting unused ones is fine but
clutters.

## Audio bus declaration

`[audio]` drives the plugin's `AUDIO_IO_LAYOUTS` constant:

```rust
const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
    AudioIOLayout {
        main_input_channels: NonZeroU32::new(<channels for default_channel_layout>),
        main_output_channels: NonZeroU32::new(<channels for default_channel_layout>),
        aux_input_ports: &[/* one NonZeroU32 per [[audio.aux_inputs]] */],
        aux_output_ports: &[/* one per [[audio.aux_outputs]] */],
        names: PortNames {
            layout: Some("<default_channel_layout, capitalized>"),
            ..PortNames::const_default()
        },
    },
];
```

Channel count from `default_channel_layout`: mono=1, stereo=2, quad=4,
five-one=6, seven-one=8.

If `[audio].latency_samples > 0`, override the default `latency_samples`
method:

```rust
fn initialize(&mut self, _audio_layout: &AudioIOLayout, buffer_config: &BufferConfig, _ctx: &mut impl InitContext<Self>) -> bool {
    self.latency_samples = <[audio].latency_samples>;
    true
}
```

## Standalone bin emission

When `formats` contains `standalone`, emit `src/bin/standalone.rs`:

```rust
use nih_plug::prelude::*;
use <crate_name>::MyPlugin;

fn main() {
    nih_export_standalone::<MyPlugin>();
}
```

The bin uses the lib's published plugin type (declared `pub` in
`lib.rs`). Standalone hosts the plugin in a small JACK / CoreAudio
window — useful for local development and CI smoke tests.

## Realtime path discipline

The `process()` callback runs on the host's audio thread. Three rules
are non-negotiable in this profile (and the basis for the deferred
`realtime_path_no_alloc` graph-check):

1. **No allocation.** No `Vec::new()`, `Box::new()`, `String::new()`,
   `format!`, `to_owned`, or any other heap call. Allocate during
   `initialize()` (which runs on the main thread, before audio starts)
   and reuse on the audio path.
2. **No locks.** No `Mutex::lock`, no `RwLock`, no blocking syscalls.
   For cross-thread state publication, use `Arc<AtomicF32>`,
   `Arc<AtomicU32>`, or a lock-free SPSC ring buffer (`rtrb`,
   `crossbeam-queue::ArrayQueue`).
3. **No logging on the hot path.** No `nih_log!`, no `println!`, no
   `defmt::info!`. If you must trace, write to a lock-free buffer that
   the editor thread drains.

When a module's `[runtime].realtime_audit = true`, codegen emits a
`#[cfg(debug_assertions)]` `assert_no_alloc` guard around `process()`
(via the `assert_no_alloc` crate, declared as an `external_dep` on the
plugin shell module). When `realtime_audit = false`, no guard emits.

## Feature gates

`module.feature_gates[]` lists Cargo features that conditionally
include the module. Emission mirrors library-api:

```rust
// feature_gates = ["midi"]
#[cfg(feature = "midi")]
mod midi_handling;

// feature_gates = ["simd", "fast-math"]
#[cfg(any(feature = "simd", feature = "fast-math"))]
mod dsp_simd;
```

An empty `feature_gates` (or omitted field) means the module is
unconditional. `module_feature_gates_resolve` verifies every name is
declared in `architecture.toml`'s `[[features]]`.

Do **not** combine `feature_gates` with `#[cfg(target_*)]` predicates
in the same `cfg` — emit them as separate `#[cfg(...)]` lines.

## Item emission

`module.items[]` carries the module's contractual surface. An item is a
**public, contractual code symbol, identified by its fully-qualified path,
described in prose — never by signature.** A method is a function: a
public inherent method is a `kind = "fn"` item named `<Type>::<method>`
(`Engine::process`), the same as a free function; the inherent `impl`
block itself is not an item, and generics are normalized out of the prefix
(`Engine::process`, not `Engine<S>::process`). A trait impl stays one
`kind = "impl"` item (`<Trait> for <Type>`) whose method bodies are
codegen-derived from the trait. Private members are not items. The shape
is **prose-first**: `description` says what the item does, NOT how it's
typed; the optional `signature_hint` is a rare escape (FFI, macro
contracts), never for ordinary functions or methods.

Sparseness rule: every item except `kind = "re_export"` must carry
`satisfies` (≥1 requirement id) — per item, though many methods may share
one coarse requirement. Plugin shell modules typically have items[]
entries for the plugin struct, the parameter struct, and any
exposed-by-bridge widgets; engine and dsp_node modules carry items[]
for their public DSP entry points (e.g. `Engine::process`,
`Engine::apply_param_changes`).

`re_export` items rarely apply on the plugin crate (the cdylib has no
Rust-callable public surface to re-export to). They DO apply on the
engine crate (which uses the library-api profile) when it re-exports
DSP primitives across module boundaries.

See "Audio-thread method bodies" below for how first-pass emission
populates audio-thread method bodies — `unimplemented!()` would
abort-crash the host on the first buffer, so the audio-thread surface
gets a different stub variant.

### Test items

An `[[items]]` entry with `test = true` is a test (D60): a `kind = "fn"`
item using `verifies = [...]` (against `nature = "test"` requirements)
instead of `satisfies`, with a `test_kind` driving emit location — `unit`
→ `#[cfg(test)] mod tests` in the module file; `integration` →
`tests/<name>.rs`; `doc` → a `///` example; `bench` / `fuzz` reserve the
slot but emit nothing in v1 (validate warns). `test_requirements_have_verifiers`
warns when a `nature = "test"` requirement has no verifier. Write a working
body from the prompt's "Test items (verification context)" section; emit
`todo!("verifies <ids>")` only when the criteria are insufficient.
Test-harness crates use an `external_deps` entry with `test_only = true`
→ `[dev-dependencies]`. Most plugin test surface lives on the engine
(`library-api`) crate — see that profile's prose for the full table.

## Cross-module dependencies

`module.depends_on[]` does not directly emit code. Rust's module system
treats `use` statements as the unit of dependency. The `depends_on`
declaration is for the spec's graph checks
(`modules_acyclic` + `layer_dependency`).

When a plugin shell module references the engine, emit
`use crate::<engine_module>::<Item>;` for single-crate layouts, or
`use <engine_crate>::<Item>;` for workspace layouts where the engine
lives in a sibling crate.

## Block and bridge contributions

Module specs in this profile typically carry block-contributed tables:

- `[nih_plug]` block — parameters, audio ports, MIDI, process
  declarative config. The block's `codegen/rust/README.md` drives
  `#[derive(Params)]` emission, range constructors, smoother
  attachment, and the export-macro selection.
- `[egui]` block — editor topology (panels, regions). The block's
  prose drives `nih_plug_egui::create_egui_editor` skeleton emission
  and per-region scaffolding.
- `bindings."egui+nih-plug"` bridge — parameter→widget bindings. The
  bridge's prose drives per-binding widget emission inside region
  loops.

Read the relevant block / bridge READMEs for those emission rules.
This profile prose covers only the architecture-level shape (crate
type, layer ordering, format/category constants, realtime discipline).

## Parameter binding model

`module.parameter_bindings[]` declares how each nih-plug parameter
translates into an engine-side write when the host changes it. The
table is **co-resident with `[nih_plug]` on the plugin_shell module**
(the shell owns the parameter surface; the engine stays framework-
agnostic and exposes only plain setters/fields). Codegen turns the
table into a single deterministic `apply_param_changes` dispatcher.

This is the third leg of the plugin-shell ↔ engine bridge: the
`egui+nih-plug` bridge wires parameters → widgets; `parameter_bindings`
wires parameters → engine writes.

### Where the dispatcher emits

Emit one method on the plugin shell:

```rust
fn apply_param_changes(&mut self) {
    // one match arm per [[parameter_bindings]] row, in declaration order
}
```

Call it from `process()` before the DSP sub-blocks run, so automation
stays sample-accurate to nih-plug's smoother output. The dispatcher
reads each parameter's current (smoothed) value off `self.params` and
performs the declared write. Because the parameter struct lives on the
shell and the engine handle (`self.engine` / `self.chip` / the field
holding the DSP state) is also on the shell, every target resolves
against `self`.

The dispatcher is **table-driven and total**: emit exactly one arm per
row, in the order the rows appear in the TOML. Do not reorder, dedupe,
or editorialize — two consecutive `gen` runs must produce a
byte-identical body. The table is the single source of truth for the
mapping; if a parameter's write changes, the spec changes, not the
generated code.

### Per-row emission

Each row reads the parameter value, applies the `value_transform`, then
performs the write keyed on `target_kind`:

```rust
// [[parameter_bindings]]
// parameter = "master_volume"
// target_kind = "method"
// target = "ChipEngine::set_master_volume"
// target_args = ["value"]
// value_transform = { kind = "db_to_amp" }
{
    let value = db_to_amp(self.params.master_volume.value());
    ChipEngine::set_master_volume(&mut self.engine, value);
}
```

`target_kind` controls the write shape:

| `target_kind` | Emission                                                            |
|---------------|---------------------------------------------------------------------|
| `method`      | `<target>(<target_args…>)`. `target` is a method path; the receiver is `&mut self.engine` (or the engine field) when `target` is a `Type::method` path. The literal token `value` in `target_args` is the transformed value. A `method` row with no `target_args` defaults to `[value]`. |
| `field`       | `<target> = value;`. `target` is a dotted path on `self` (`self.bypassed`, `self.voice.lfo1.pmd`). No `target_args`. |
| `custom`      | `self.<target>(value);`. `target` names a hand-authored fn on the module; its `description` tells you (and the author) what it must do. Use for context-dependent or multi-field writes the declarative forms can't express. |

The `target` and `target_args` tokens are emitted **verbatim** — the
spec does not type-check them; the Rust compiler does. Read the
parameter value with `self.params.<id>.value()` (nih-plug's smoothed
accessor); bind it to a local `value`, apply the transform, then write.

### Value transforms (D59)

Six named transforms plus a `custom` escape hatch. Each rewrites the
`value` local before the write:

| `value_transform`                   | Emitted `value` expression                          |
|-------------------------------------|------------------------------------------------------|
| `{ kind = "identity" }` (default)   | `self.params.<id>.value()` (unchanged)               |
| `{ kind = "clamp", min, max }`      | `self.params.<id>.value().clamp(<min>, <max>)`       |
| `{ kind = "scale", factor }`        | `self.params.<id>.value() * <factor>`                |
| `{ kind = "enum_to_u8" }`           | `self.params.<id>.value() as u8`                     |
| `{ kind = "db_to_amp" }`            | `db_to_amp(self.params.<id>.value())`                |
| `{ kind = "custom", fn = "<name>" }`| `<name>(self.params.<id>.value())`                   |

Omitting `value_transform` is `identity`. `db_to_amp` is the one named
transform whose math isn't trivially inline — emit a single free fn
once per crate and reuse it:

```rust
#[inline]
fn db_to_amp(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}
```

`custom` transforms reference a hand-authored pure fn `fn(value) -> _`
named by `fn`; emit the call, not the body — the author writes the body.
A `custom` *transform* reshapes the value before a declarative write; a
`custom` *target_kind* performs the whole write itself. They compose:
a `custom` target ignores `value_transform` semantics beyond passing the
(optionally transformed) value into the named fn.

### Worked example

```rust
fn apply_param_changes(&mut self) {
    {
        let value = db_to_amp(self.params.master_volume.value());
        ChipEngine::set_master_volume(&mut self.engine, value);
    }
    {
        let value = self.params.out_assign.value() as u8;
        ChipEngine::set_out_assign(&mut self.engine, value);
    }
    {
        let value = self.params.bypass.value();
        self.bypassed = value;
    }
}
```

That body is a pure projection of the `[[parameter_bindings]]` table —
nothing in it is invented. Adding a parameter without adding its
binding row trips the `parameters_have_bindings` warning at validate
time, so the dispatcher never silently drops a parameter.

## Audio-thread method bodies

Audio-thread methods on the plugin shell — `process`, `process_buffers`,
and any closure body invoked on the audio thread — emit a
**silence-and-log** body in the first-pass `gen` output: write zeros
into the output buffer, fire one diagnostic `nih_log!` on first
invocation, and return the normal process status. Non-audio-thread
code (`initialize`, `reset`, editor / state plumbing, hand-authored
helpers that don't run on the audio thread) emits `unimplemented!()`
so the panic surfaces the missing implementation loudly during
authoring.

The split is load-bearing: `unimplemented!()` on the audio thread
aborts the host (the user's DAW). Silence-and-log on the audio thread
keeps the host alive long enough to verify the bundle loads and the
stub surface is wired up correctly. Everywhere else, `unimplemented!()`
is the fastest gap-finder available — let the panic surface the gap.

### Audio-thread classification

The following nih-plug `Plugin` trait methods are audio-thread by
definition. When `gen` produces an `impl Plugin` block on the plugin
shell module, the bodies of these methods take the silence-and-log
template:

- `process`
- `process_buffers`
- `task_executor`'s emitted closure body, when the closure is invoked
  on the audio thread (most plugins return a no-op closure here, in
  which case the closure body itself is silence-and-log)

The list is intentionally short — nih-plug's `Plugin` trait keeps
audio-thread method signatures in a tight cluster. New audio-path
methods that nih-plug adds in future revisions extend this list when
the framework dependency bumps. AU support, when authored, supplies
its own audio-path method list in a sibling section here.

**Hand-authored DSP-node helpers** (filter step functions, oscillator
render fns, voice render entry points) aren't trait methods, so they
aren't reachable from the list above. The LLM driving `gen` should
classify them as audio-thread by reading their semantics — anything
called from inside `process` / `process_buffers` is audio-thread, and
that body should also take the silence-and-log template. Helpers that
the LLM cannot confidently classify default to `unimplemented!()`;
authors then either rerun `gen` with explicit prose ("the
`render_block` helper runs on the audio thread") or hand-edit the
emitted body.

### Silence-and-log emit template

For each audio-thread method, the body is:

```rust
fn process(
    &mut self,
    buffer: &mut Buffer,
    _aux: &mut AuxiliaryBuffers,
    _context: &mut impl ProcessContext<Self>,
) -> ProcessStatus {
    #[cfg(debug_assertions)]
    {
        static FIRED: std::sync::atomic::AtomicBool =
            std::sync::atomic::AtomicBool::new(false);
        if FIRED
            .swap(true, std::sync::atomic::Ordering::Acquire)
            == false
        {
            nih_log!("[stub] process() reached — silence-and-log stub");
        }
    }
    for channel_samples in buffer.iter_samples() {
        for sample in channel_samples {
            *sample = 0.0;
        }
    }
    ProcessStatus::Normal
}
```

The mechanical pieces:

1. **Log surface is `nih_log!`** — the plugin crate already depends on
   nih-plug, so `nih_log!` carries no added Cargo surface. Do not
   reach for `log::info!`, `println!`, or `eprintln!` — those would
   force every plugin to drag in `env_logger` or emit nothing.

2. **Once-only via `AtomicBool::swap`** — per-call logging would flood
   the host log within a buffer's worth of frames at any meaningful
   sample rate. The static `FIRED` flag flips on the first call and
   subsequent calls take the relaxed-load fast path (the `swap`
   returns true immediately).

3. **Buffer write is silence** — write `0.0` into every sample of
   every channel of `buffer`. For methods that take an output buffer
   not named `buffer` (rare in nih-plug today), substitute the
   parameter name. The point is that the bundle produces audible
   silence rather than uninitialized data.

4. **Non-buffer return values default to `Default::default()`**.
   Audio-thread methods returning non-`()` types (e.g. a status enum,
   a `bool`, a numeric tail length) return their `Default` value as
   the universal stub. Audio-thread methods returning a non-`Default`
   type need a hand-authored body — the LLM should emit a `todo!()`
   in that case so the gap is loud at compile time rather than at
   audio-thread invocation time.

5. **`runtime.realtime_audit = true` gates the once-only log on
   `cfg!(debug_assertions)`.** A user who has opted into the
   no-allocation / no-logging audit on the audio thread has explicitly
   forbidden release-build logging there. The `#[cfg(debug_assertions)]`
   wrapper around the whole logging block keeps the diagnostic loud
   during local development and silent in release-built bundles. The
   `AtomicBool` machinery itself is inside that `cfg`, so realtime-audit
   release builds carry zero stub overhead beyond the empty function
   body and the silence write.

   When `runtime.realtime_audit = false` (the default), the
   `#[cfg(debug_assertions)]` wrapper is still applied — the goal is
   "loud in dev, silent in release" universally. The `realtime_audit`
   flag changes nothing at the silence-and-log site itself; it only
   adds the existing `assert_no_alloc` guard around `process()`, which
   sees the silence-and-log body's lack of allocation just fine.

### `unimplemented!()` retention list

The following methods / surfaces always emit `unimplemented!()`,
regardless of trait membership:

- `initialize` — runs on the main thread before audio starts. Panic
  here surfaces the gap loudly in the host's plugin-load dialog.
- `reset` — runs on the main thread between transport stops/starts.
  Same reasoning.
- `editor` — returns `Option<Box<dyn Editor>>`; emit `None` rather
  than panic-on-call (an editor stub that panics would crash the
  host on UI open). Emit `Some(Box::new(no_op_editor))` only when
  the egui block is on the module; otherwise the body is `None`.
- `extra_state` / `load_state` — preset / state plumbing on the main
  thread. `unimplemented!()` surfaces the gap when the host attempts
  preset save/restore.
- Any non-audio-thread fn item.

The mechanical rule: only audio-thread code gets silence-and-log.
Everything else takes `unimplemented!()` and lets the panic surface
the missing implementation during development. The audio thread is
the one place a panic is strictly unrecoverable; everywhere else a
panic is the fastest gap-finder available.

### What the silence-and-log template does NOT do

- **Metering / state-publishing writes.** Real plugins publish meter
  values, scope frames, and other audio-thread state to the editor
  via `AtomicCell` writes or lock-free ring buffers. Silence-and-log
  writes nothing back to the editor. The consumer side (the editor)
  is also a stub at first-pass gen, so there's no observable benefit
  to emitting them; the LLM should hand-author meter writes when the
  editor is wired up.
- **MIDI emission.** `process_buffers`'s `context.send_event(...)`
  surface is silent under the template. Plugins whose primary value
  is generated MIDI (sequencers, arps) need the LLM to hand-author
  the emission.
- **AU format support.** Standalone / VST3 / CLAP only; AU ships its
  own audio-thread surface that this section will extend when AU
  support lands.
