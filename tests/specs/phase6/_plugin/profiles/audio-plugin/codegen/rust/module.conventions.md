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

For multi-crate workspaces (the recommended layout per Phase 8.6 +
Phase 15 spike §0): the engine lives in a sibling member crate using
the `library-api` profile (`no_std`-eligible, no plugin concerns); the
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
hierarchy. The four-tier ordering (D44):

| Layer            | Typical content                                          | Allowed deps           |
|------------------|----------------------------------------------------------|------------------------|
| `host_interface` | Reserved — nih-plug-provided types. No user modules.     | —                      |
| `plugin_shell`   | `impl Plugin for X`, parameter struct, editor wiring     | `engine`, `dsp_node`   |
| `engine`         | Voice management, preset state, DSP coordination         | `dsp_node`             |
| `dsp_node`       | Leaf DSP units (oscillators, filters, envelopes)         | (leaf — no deps)       |

The `layer_dependency` graph check rejects upward dependencies. v1
restricts each plugin crate to **one** `plugin_shell` module — multi-shell
per crate is bounded-deferred (spike §7). Modules at `host_interface`
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

## Per-format constants (D46)

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
`realtime_path_no_alloc` graph-check, D48):

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

## Item emission (Phase 14)

`module.items[]` carries the module's contractual surface entries — one
per top-level Rust construct that has spec significance (typically
because it satisfies a requirement). The shape is **prose-first**: each
item carries a `description` of what it does, NOT a signature mirror
of how it's typed. The optional `signature_hint` field exists as an
escape hatch for cases where prose alone is insufficient (FFI
declarations, macro contracts, generics-heavy trait impls); avoid it
for ordinary functions and types.

Sparseness rule: every item except `kind = "re_export"` must carry
`satisfies` (≥1 requirement id). Plugin shell modules typically have
items[] entries for the plugin struct, the parameter struct, and any
exposed-by-bridge widgets; engine and dsp_node modules carry items[]
for their public DSP entry points.

`re_export` items rarely apply on the plugin crate (the cdylib has no
Rust-callable public surface to re-export to). They DO apply on the
engine crate (which uses the library-api profile) when it re-exports
DSP primitives across module boundaries.

**Skeleton mode is library-api only (D49).** A plugin crate's
`process()` callback runs on the audio thread; emitting
`unimplemented!()` there would abort-crash the host. The engine crate
(library-api) gets skeleton mode normally; the plugin crate (this
profile) is excluded from `gen --skeleton` until silence-and-log stubs
land (bounded follow-up).

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

- `[nih_plug]` (Phase 15-C block) — parameters, audio ports, MIDI,
  process declarative config. The block's
  `codegen/rust/README.md` drives `#[derive(Params)]` emission, range
  constructors, smoother attachment, and the export-macro selection.
- `[egui]` (Phase 15-D block) — editor topology (panels, regions). The
  block's prose drives `nih_plug_egui::create_egui_editor` skeleton
  emission and per-region scaffolding.
- `bindings."egui+nih-plug"` (Phase 15-E bridge) — parameter→widget
  bindings. The bridge's prose drives per-binding widget emission
  inside region loops.

Read the relevant block / bridge READMEs for those emission rules.
This profile prose covers only the architecture-level shape (crate
type, layer ordering, format/category constants, realtime discipline).
