# audio-plugin — implementation guidance

Rules an agent follows when it writes or edits code in an `audio-plugin` package. No check verifies them; review
does. Framework rules (parameters, editor) live in the guidance of the `nih-plug`, `egui` and `egui+nih-plug` units.

## Crate shape

- The plugin crate builds a `cdylib`: VST3 and CLAP load a C-ABI library.
- The standalone format is a `[[bin]]` target, not a Cargo feature. The bin links the crate's own library, so the
  manifest must list both: `crate-type = ["cdylib", "lib"]`. With `["cdylib"]` alone the bin does not compile
  (`cannot find module or crate`).
- `Cargo.toml` is the truth for dependencies, features and profiles. The spec does not copy it.
- Prefer a workspace: the engine in a sibling `library-api` crate with no plugin concerns, the plugin crate on top.

## Layers

- `plugin_shell` → `engine` → `dsp_node`; dependencies point down only (`layer_dependency` checks the spec,
  `internal_edges_declared` checks the imports).
- One `plugin_shell` module per crate. It owns the `Plugin` implementation, the parameter struct and the editor
  wiring. No user module sits at `host_interface`.
- The engine stays framework-free: plain setters and fields, no plugin types.

## The `[plugin]` and `[audio]` tables

- The plugin's name, vendor, URL, e-mail and version constants carry the `[plugin]` values. An omitted URL or e-mail
  is the empty string.
- `[plugin].category` sets the VST3 category string and the CLAP feature list together; they must agree.
- Export one format macro per entry in `[plugin].formats`. The standalone format exports from the bin, not the
  library.
- The main bus channel count follows `[audio].default_channel_layout`; one aux port per declared aux bus; a
  non-zero `latency_samples` is reported to the host.

## Parameter bindings

- `[[parameter_bindings]]` is the whole parameter → engine mapping. One write per row, in table order, applied in
  `process` before the DSP runs, from the smoothed parameter value.
- Change the table, not the dispatcher, when a write changes. A parameter with no row trips
  `parameters_have_bindings`.

## The audio thread

`process`, `process_buffers` and every function they call run on the host's audio thread.

- No allocation, no locks, no blocking calls, no logging in the steady state. Allocate in `initialize`; publish state
  to the editor through atomics or a lock-free queue.
- A panic on the audio thread kills the user's host. An unfinished audio-thread function therefore writes silence to
  its output, logs one diagnostic through the framework's logger on its first call (debug builds only), and returns
  the normal status — it never calls `unimplemented!`, `todo!` or `panic!`.
- Code off the audio thread (`initialize`, `reset`, state save and load) may panic while unfinished: the panic finds
  the gap fastest. An unfinished editor returns no editor; it does not panic.
- `[runtime].panic_handler = "abort"` is the profile's rule for the release profile. Cargo does not enforce it until
  the manifest says `panic = "abort"`.
