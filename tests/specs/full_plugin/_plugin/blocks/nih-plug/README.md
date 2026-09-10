# nih-plug block

Audio-plugin framework block targeting
[nih-plug](https://github.com/robbert-vdh/nih-plug) — an opinionated
Rust framework that builds VST3 / CLAP / standalone artifacts from a
single crate. Contributes a namespaced `[nih_plug]` table on `Module`
covering the plugin shell's declarative surface: **parameters**,
**audio ports**, **MIDI configuration**, and **process-callback
metadata**.

## When to use this block

Inline `nih-plug` when your project:

- Ships an audio plugin via VST3, CLAP, or standalone formats (the
  formats this block + the audio-plugin profile know how to drive).
- Uses Rust as the implementation language.
- Has parameters automated by the host, audio I/O the host wires up,
  optional MIDI input/output, and a realtime `process()` callback.

Don't use this block if:

- You're building an internal DSP library — `library-api` is the right
  profile and you don't need the plugin shell at all.
- You're targeting an audio framework other than nih-plug (e.g. a
  direct VST3 SDK port, JUCE, AudioWorklet). Those would land as
  separate blocks (`vst3-sdk-rs`, etc.); the conceptual shape overlaps
  but the codegen mappings differ.

This block is silent on UI. Pair with a UI block (`egui` ships in v1)
plus the matching bridge (`egui+nih-plug`) for parameter→widget
rendering. You CAN use nih-plug without an editor —
hosts fall back to a generic parameter list — but most plugins ship
a custom editor.

## What this block contributes

Schema additions on `Module`:

```toml
[nih_plug]
# (no top-level fields in v1; all content in subtables)

[[nih_plug.parameters]]
id      = "gain"
name    = "Gain"
kind    = "float"        # float | int | bool | enum
min     = -30.0
max     = 30.0
default = 0.0
unit    = "dB"
smoother    = "logarithmic"   # none | linear | logarithmic | exponential
smoother_ms = 50

[[nih_plug.parameters]]
id   = "filter_type"
name = "Filter"
kind = "enum"
variants = ["lowpass", "highpass", "bandpass", "notch"]
default  = "lowpass"

[[nih_plug.audio_ports]]
name          = "main_in"
direction     = "input"        # input | output
channel_count = 2
kind          = "main"         # main | aux | sidechain

[nih_plug.midi]
input_level  = "midi_ccs"   # none | basic | midi_ccs
output_level = "none"       # (midi_ccs delivers CC + pitch-bend events)

# Optional — only when the plugin handles SysEx (e.g. patch dumps).
# Requires input_level or output_level at "basic" or higher.
[nih_plug.midi.sysex]
max_message_len = 4104        # bytes, incl. 0xF0…0xF7 (DX7 bank dump)
message_type    = "Dx7Dump"   # optional; default <PluginType>SysEx

[nih_plug.process]
mode             = "realtime"  # realtime | offline_capable
block_size_max   = 4096        # optional
```

Reduction kinds (after inline):

- `nih-plug.Parameter` (extends `Module`) — one per
  `[[nih_plug.parameters]]`. Qualified name is
  `<ModuleName>.<parameter_id>`.
- `nih-plug.AudioPort` (extends `Module`) — one per
  `[[nih_plug.audio_ports]]`.
- `nih-plug.MidiConfig` (extends `Module`) — at most one per module
  (the `[nih_plug.midi]` singleton). Qualified name is
  `<ModuleName>.midi`. Its optional `sysex` subtable is NOT surfaced as
  graph attributes — like `ProcessConfig`, it's a nameless config
  singleton that only codegen reads.
- `ProcessConfig` is intentionally NOT materialized as an entity —
  it carries no name and no graph check needs to reach into it;
  codegen prose reads it directly from the module's TOML.

Codegen prose (`codegen/rust/README.md`) describes how each shape
maps to Rust: parameters become `#[derive(Params)]` fields with
`*Param::new(...)` constructors and smoother attachment; audio ports
populate the plugin's `AUDIO_IO_LAYOUTS` constant; MIDI flags drive
`MIDI_INPUT` / `MIDI_OUTPUT` constants; process metadata drives
preallocation patterns in `initialize()`.

## Sharp-test rationale

- **Concreteness:** ✓ — names a specific Rust framework (`nih_plug`),
  not the abstract concept of "audio plugin framework."
- **Codegen-completeness:** ✓ — given a conforming TOML, the codegen
  prose produces working Rust source: the parameter struct, the
  plugin impl skeleton with all required constants, the export
  macros at crate root, an `initialize()` shell. The user fills in
  `process()` body and any custom state handling. No step requires
  "depends on which X."
- **No cross-block reach:** ✓ — codegen prose explicitly excludes
  UI crates (egui / iced / vizia) and DSP libraries. UI rendering is
  the job of the `nih-plug+<ui-block>` bridge; DSP code lives in the
  user's engine module(s) (or a sibling `library-api` crate).

## Deferred (revisit triggers per §12)

- **Background tasks.** `type BackgroundTask` is `()` in v1; nih-plug
  supports per-plugin background-thread work for things like preset
  loading. Promote when authoring a plugin with non-trivial deferred
  work. (This is the natural home for decoding a SysEx patch dump off
  the audio thread — see `[nih_plug.midi.sysex]` below, whose codegen
  hands the raw frame to the engine but leaves the parse to the user.)
- **Custom state save/load.** `extra_state` / `load_state` for state
  beyond the parameter set is hand-authored in the plugin shell
  module today. Promote to a typed `[nih_plug.state]` subtable when
  more than two plugins author the same hand-rolled shape.
- **Per-platform realtime audit.** Schema captures `process.mode`;
  the `realtime_audit` boolean lives at the architecture level. A
  future graph-check primitive that statically detects alloc on the
  audio path is a bounded follow-up, gated on dogfood evidence
  (`realtime_path_no_alloc`).
- **Polyphonic-mod id uniqueness check.** `poly_modulation_id` is a
  CLAP feature that needs project-wide unique ids; v1 doesn't
  graph-check this. Promote when CLAP-poly-mod authoring lands.
- **Wider parameter kinds.** `kind = "stepped-float"` (a float
  snapped to a fixed grid; nih-plug supports it) and other variants
  are bounded follow-ups.
