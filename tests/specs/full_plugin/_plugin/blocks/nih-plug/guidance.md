# nih-plug — implementation guidance

Rules an agent follows when it writes or edits code for a module with a `[nih_plug]` table: parameters, audio ports,
MIDI and the process callback of a nih-plug plugin. No check verifies them; review does.

## Scope

- nih-plug is not on crates.io: the project pins a git revision in `Cargo.toml` and records the pin in its README.
- No UI or DSP crate from this block. The editor belongs to a UI bridge, not to this block's code.

## Parameters

- Each `[[nih_plug.parameters]]` entry is one field of the plugin's derived parameter struct, named by its `id`.
- The field's persisted id attribute equals the spec's `id` exactly and never changes: hosts store it in saved
  projects, so a change breaks every saved session.
- The display label is `name`; `default`, `min` and `max` set the value and range. A unit string is attached with a
  leading space.
- `kind = "enum"` has a Rust enum beside the struct, one variant per `variants[]` entry in order.
- The range follows the smoother curve: linear or none gives a linear range; logarithmic and exponential give a
  skewed range (negative and positive skew).
- The smoothing style matches `smoother`, with `smoother_ms` as its time; `none` attaches no smoother.
  `poly_modulation_id` is attached only when set.

## The plugin

- The CLAP id is the vendor and name, lower case, spaces as hyphens — stable once released.
- Sample-accurate automation stays on unless the plugin documents block-boundary handling.
- The audio layout combines `[audio]`'s main channel count with `[[nih_plug.audio_ports]]`: aux and sidechain ports
  by direction, each with its `channel_count`. A `main` port must agree with `[audio]`.
- MIDI input and output configs follow `input_level` and `output_level` one to one. CC and pitch-bend events arrive
  only at `midi_ccs`.

## process()

- Read parameters through their smoothers on the audio thread, never the raw value (zipper noise).
- Drain note, CC and SysEx events at the start of the block.
- Return the keep-alive status only while a tail (decay, release) must keep running.

## SysEx

- With a `[nih_plug.midi.sysex]` subtable, the plugin has a SysEx message type named `message_type`. Its buffer is a
  fixed array of `max_message_len` bytes, and it holds the raw frame, start and end bytes included. Receiving needs
  `input_level` of `basic` or above; sending needs the same of `output_level`.
- Recognising a message only copies the frame; it returns nothing for SysEx the plugin does not handle. The real
  decode happens in the engine, off the audio thread.

## Lifecycle and state

- With `[nih_plug.process].block_size_max`, preallocate working buffers in `initialize`, and cache the sample rate
  there. `initialize` fails only for unrecoverable conditions.
- The derived parameter struct saves and restores parameter state; other state needs hand-written save and load.
