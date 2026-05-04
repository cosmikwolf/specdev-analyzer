# nih-plug — Rust codegen conventions

These conventions describe how a `nih-plug` block table on a module
spec maps to Rust source. They layer on top of the audio-plugin
profile's `module.conventions.md`; concerns specific to the nih-plug
framework (parameter-derive, range constructors, smoothers, export
macros, process-callback shape) live here.

The block targets [nih-plug](https://github.com/robbert-vdh/nih-plug)
as its concrete framework. This block is silent on UI — pair with a
UI block (e.g. `egui`) plus the matching bridge for parameter →
widget rendering.

## Crate dependencies

This block contributes one core crate:

```toml
nih_plug = { git = "https://github.com/robbert-vdh/nih-plug.git", rev = "<pin>" }
```

nih-plug is not on crates.io; pin a specific revision in the project's
`architecture.toml` `[[external_deps]]` and document the pin in the
plugin's README. Suggested first-time pin: a recent tag from the
nih-plug repo at the time you start a project.

Per-format extras come from the same crate:

- VST3: nih-plug exports VST3 via the `assert_process_allocs` /
  `nih_export_vst3!` machinery — no separate VST3 SDK pull-in needed.
- CLAP: same crate, `nih_export_clap!`.
- Standalone: same crate, `nih_export_standalone()` from the bin.

**Do NOT** add UI crates or DSP libraries from this block — those are
the project's choice (specced via the engine member crate's
`external_deps[]` or the relevant UI block).

## Parameter struct emission

Each `[[nih_plug.parameters]]` entry becomes one field on the plugin's
`#[derive(Params)]` struct. The struct itself is emitted in the
plugin shell module:

```rust
use nih_plug::prelude::*;
use std::sync::Arc;

#[derive(Params)]
pub struct MyPluginParams {
    #[id = "gain"]
    pub gain: FloatParam,

    #[id = "voices"]
    pub voices: IntParam,

    #[id = "bypass"]
    pub bypass: BoolParam,

    #[id = "filter_type"]
    pub filter_type: EnumParam<FilterType>,
}
```

The struct name is `<PluginShellType>Params` by convention. The field
name is the parameter's `id` (no transformation). The `#[id = "..."]`
attribute MUST match the parameter's `id` field exactly — hosts persist
this id in projects, so any deviation breaks save-state compatibility.

For `kind = "enum"`, codegen also emits a Rust enum next to the
parameter struct — variants come verbatim from the `variants[]` array,
mapped to `PascalCase` Rust idents:

```rust
#[derive(Enum, PartialEq, Eq, Clone, Copy)]
pub enum FilterType {
    Lowpass,
    Highpass,
    Bandpass,
    Notch,
}
```

`#[derive(Enum)]` from `nih_plug::params::enums::Enum` is the load-
bearing one — it provides the surfaces nih-plug uses for serialization
+ host listing. The other derives are conventional.

## Range constructors per kind

The parameter's `kind` drives the constructor:

| kind   | nih-plug type       | Range constructor                                                    |
|--------|---------------------|----------------------------------------------------------------------|
| float  | `FloatParam`        | `FloatRange::Linear { min, max }` (smoother = linear / none)         |
| float  | `FloatParam`        | `FloatRange::Skewed { min, max, factor }` (smoother = log / exp)     |
| int    | `IntParam`          | `IntRange::Linear { min, max }`                                      |
| bool   | `BoolParam`         | (no range; default only)                                             |
| enum   | `EnumParam<E>`      | (no range; default variant only)                                     |

Smoother-curve mapping (the load-bearing skew table):

| smoother      | factor                                              |
|---------------|-----------------------------------------------------|
| linear / none | `FloatRange::Linear`                                |
| logarithmic   | `FloatRange::Skewed { factor: FloatRange::skew_factor(-2.0) }`         |
| exponential   | `FloatRange::Skewed { factor: FloatRange::skew_factor(2.0) }`          |

The init pattern for one float parameter with a logarithmic smoother:

```rust
impl Default for MyPluginParams {
    fn default() -> Self {
        Self {
            gain: FloatParam::new(
                "Gain",
                0.0,
                FloatRange::Skewed {
                    min: -30.0,
                    max:  30.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" dB")
            .with_smoother(SmoothingStyle::Logarithmic(50.0)),

            voices: IntParam::new(
                "Voice Count",
                8,
                IntRange::Linear { min: 1, max: 32 },
            ),

            bypass: BoolParam::new("Bypass", false),

            filter_type: EnumParam::new("Filter", FilterType::Lowpass),
        }
    }
}
```

The display name passed as the first arg to each `*Param::new` is the
parameter's `name` field (the human-readable label). The default value
comes from `default`. `min` / `max` populate the range. `unit` (when
set) attaches via `.with_unit(" <unit>")` (note the leading space — a
nih-plug convention; the host concatenates `value` + unit).

`with_smoother` uses `smoother_ms` for the time constant. Map the
nih-plug `SmoothingStyle` per smoother enum:

| smoother    | SmoothingStyle                                  |
|-------------|-------------------------------------------------|
| none        | omit `.with_smoother` entirely                  |
| linear      | `SmoothingStyle::Linear(<smoother_ms>.0)`       |
| logarithmic | `SmoothingStyle::Logarithmic(<smoother_ms>.0)`  |
| exponential | `SmoothingStyle::Exponential(<smoother_ms>.0)`  |

`poly_modulation_id` (CLAP polyphonic modulation) attaches via
`.with_poly_modulation_id(<id>)` after the smoother. Skip when unset.

## Plugin trait skeleton

The plugin shell module emits one `impl Plugin for <PluginType>`:

```rust
pub struct MyPlugin {
    params: Arc<MyPluginParams>,
}

impl Default for MyPlugin {
    fn default() -> Self {
        Self {
            params: Arc::new(MyPluginParams::default()),
        }
    }
}

impl Plugin for MyPlugin {
    const NAME: &'static str    = /* [plugin].name */;
    const VENDOR: &'static str  = /* [plugin].vendor */;
    const URL: &'static str     = /* [plugin].url, or "" */;
    const EMAIL: &'static str   = /* [plugin].email, or "" */;
    const VERSION: &'static str = /* [plugin].version */;

    const VST3_CATEGORIES: &'static str            = /* category-table lookup */;
    const CLAP_FEATURES: &'static [ClapFeature]    = /* category-table lookup */;
    const CLAP_ID: &'static str                    = /* derived: "<vendor>.<name>" lowercased + dashed */;
    const CLAP_DESCRIPTION: Option<&'static str>   = /* [plugin] description if present, else None */;
    const CLAP_MANUAL_URL: Option<&'static str>    = None;
    const CLAP_SUPPORT_URL: Option<&'static str>   = None;

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        // populated from [audio] + [[nih_plug.audio_ports]] (see below)
    ];

    const MIDI_INPUT: MidiConfig  = /* [nih_plug.midi].input ? Basic : None */;
    const MIDI_OUTPUT: MidiConfig = /* [nih_plug.midi].output ? Basic : None */;

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // user code
        ProcessStatus::Normal
    }
}
```

`URL` / `EMAIL`: emit literal `""` when the spec omits them. nih-plug
requires the constants to exist; empty string surfaces as "no link" in
the host.

`CLAP_ID` derivation: lowercase `[plugin].vendor` + `.` + lowercase
`[plugin].name` with spaces replaced by hyphens (e.g.
`"MyCompany Acme Plugin"` → `"mycompany.acme-plugin"`). nih-plug
recommends a stable bundle-id-style string here.

`SAMPLE_ACCURATE_AUTOMATION = true` is the conventional default; flip
to `false` only when the plugin documentably handles automation at
block boundaries only.

## AUDIO_IO_LAYOUTS population

The audio layout array combines `[audio]` (architecture) with
`[[nih_plug.audio_ports]]` (this block):

- Main input + main output channel counts come from `[audio]
  default_channel_layout` (mono=1, stereo=2, quad=4, five-one=6,
  seven-one=8) — emit as the layout's `main_input_channels` /
  `main_output_channels`.
- Each `[[nih_plug.audio_ports]]` with `kind = "aux"` or
  `kind = "sidechain"` becomes an entry in `aux_input_ports` /
  `aux_output_ports` (split by `direction`). Each port's
  `channel_count` populates a `NonZeroU32::new(<count>)`.
- Port `name` populates the `PortNames` struct entry under the
  matching position; lex-sort to keep emission byte-stable.

When `[[nih_plug.audio_ports]]` is empty (the typical case), emit a
single `AudioIOLayout` with main I/O only. When ports declare
`kind = "main"`, codegen verifies they match `[audio]` (mono pair vs
stereo pair, etc.); a mismatch is a codegen-time warning.

## Export macro selection

`[plugin].formats` (read from architecture) drives which
`nih_export_*!` macros emit. The macros live at the **crate root** in
`src/lib.rs`, NOT in this module:

```rust
// src/lib.rs
mod my_plugin;
use my_plugin::MyPlugin;

nih_export_vst3!(MyPlugin);     // when "vst3" in [plugin].formats
nih_export_clap!(MyPlugin);     // when "clap" in [plugin].formats
```

Standalone is NOT exported here — the standalone format gets a
`[[bin]]` target whose source emits `nih_export_standalone()` per the
profile's `module.conventions.md` §"Standalone bin emission."

Emit one macro per declared format, in declaration order. **Do NOT**
emit unused macros (e.g. `nih_export_vst3!` when `vst3` isn't in
`formats`) — nih-plug compiles unused macros fine, but they pull in
VST3 SDK code that bloats the binary.

## process() body shape

The `process()` callback runs on the host's audio thread under the
realtime contract spelled out in the audio-plugin profile prose
(no alloc, no lock, no log on the hot path). This block adds two
nih-plug-specific patterns:

```rust
fn process(
    &mut self,
    buffer: &mut Buffer,
    _aux: &mut AuxiliaryBuffers,
    context: &mut impl ProcessContext<Self>,
) -> ProcessStatus {
    // 1. Drain MIDI events at the start of the block (when MIDI_INPUT is set).
    while let Some(event) = context.next_event() {
        match event {
            NoteEvent::NoteOn { timing, voice_id, channel, note, velocity, .. } => {
                // dispatch to engine
            }
            NoteEvent::NoteOff { .. } => { /* ... */ }
            _ => {}
        }
    }

    // 2. Iterate samples or per-channel slices, using parameter
    //    smoothed values via .smoothed.next() on each parameter.
    for mut channel_samples in buffer.iter_samples() {
        let gain = self.params.gain.smoothed.next();
        for sample in channel_samples.iter_mut() {
            *sample *= util::db_to_gain(gain);
        }
    }

    ProcessStatus::Normal
}
```

Two rules:
- **Always read smoothed values via `.smoothed.next()`** — the raw
  `.value()` returns the latest host-set value without smoothing.
  Reading raw on the audio thread produces zipper noise.
- **Match on `NoteEvent` exhaustively** — `_ => {}` is fine for v1,
  but logging unhandled events to a lock-free buffer (drained by the
  editor thread) is a good follow-up pattern.

`ProcessStatus::Normal` is the conventional return; use
`ProcessStatus::KeepAlive` only when the plugin's tail (reverb decay,
note release) needs to keep running after note-off. nih-plug uses
this to prevent the host from sleeping the plugin.

## initialize() for preallocation

When `[nih_plug.process].block_size_max` is set, the plugin should
preallocate any working buffers in `initialize()`:

```rust
fn initialize(
    &mut self,
    _audio_layout: &AudioIOLayout,
    buffer_config: &BufferConfig,
    _context: &mut impl InitContext<Self>,
) -> bool {
    let max_block = /* [nih_plug.process].block_size_max */;
    self.scratch_buffer.resize(max_block as usize, 0.0);
    // any other one-time setup
    true
}
```

`buffer_config.sample_rate` is available here; cache it on the plugin
struct if the DSP needs it (avoid reading per-sample). Returning
`false` aborts plugin loading — reserve for unrecoverable conditions
(buffer-allocation failure, host-config mismatch).

## State save/load (default behavior)

`#[derive(Params)]` provides automatic state serialization for the
parameter set — no extra code needed. Custom state (preset name,
oversampling-factor selection that isn't a parameter) requires
implementing `extra_state`/`load_state` manually; out of scope for
v1 codegen prose. When a project needs custom state, add the impl
manually in the plugin shell module.

## Interaction with audio-plugin profile conventions

The audio-plugin profile's `module.conventions.md` covers crate-shape
(cdylib + standalone bin), per-format constants (VST3_CATEGORIES /
CLAP_FEATURES table per `[plugin].category`), and realtime path
discipline. This block extends those:

- The plugin shell module's struct + `#[derive(Params)]` struct +
  `impl Plugin for X` come from this block (above).
- The constants table lookup (category → VST3_CATEGORIES /
  CLAP_FEATURES) comes from the profile prose.
- The export macros at `src/lib.rs` come from this block.
- The realtime contract on `process()` comes from the profile prose;
  this block reinforces with `.smoothed.next()` and the no-`_ => {}`
  +alloc rule above.

**Do NOT** emit a UI editor here — that's the UI block's
responsibility (e.g. `egui` block, paired via the `egui+nih-plug`
bridge). The plugin trait's `editor()` method is implemented by the
bridge's prose, not here.
