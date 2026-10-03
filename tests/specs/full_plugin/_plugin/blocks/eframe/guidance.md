# eframe — implementation guidance

Rules an agent follows when it writes or edits code for a module with an `[eframe]` table: the native application
shell (window, repaint policy, persisted state, extra viewports). No check verifies them; review does.

## Dependencies and the egui path

- Reach egui through eframe's re-export; never add egui as a separate dependency (two eguis whose types do not
  unify fail far from the cause).
- The crate root re-exports eframe's egui once (`pub use eframe::egui;`), so any UI code resolves egui through
  `crate::egui`. eframe's egui uses the newer panel API generation (panels shown into a `Ui`).
- Persisted entries need eframe's persistence feature; without it the storage helpers do not exist.

## The App trait

- Painting goes in the trait's required painting method; state that arrives from elsewhere goes in the optional
  logic method, which also runs while the window is hidden, so nothing queues without bound.
- Check the trait's method names against the eframe version in `Cargo.toml`. A method with an old name compiles as
  an inherent method, is never called, and the window paints blank.
- No blocking work in either method: both run on the UI thread.

## Window

- The `[eframe]` scalars become the native options and the root viewport builder. `app_name` is not a window field:
  it is the run call's first argument and decides where persisted state lives. `window_title` is the window's text.
- Sizes are logical points as floats. With no `min_size` in the spec, set no minimum.

## Repaint

- `reactive`: nothing extra. A producer off the UI thread holds a clone of the context and requests a repaint when
  it has work — same latency, zero idle CPU.
- `continuous`: request a repaint each frame, at the end of the logic method. `interval`: request one after
  `repaint_interval_ms`.
- Do not choose `continuous` because state changes off-thread, and do not request a repaint on every painting pass:
  both spin a core forever.

## Persistence

- Each `[[eframe.persisted]]` entry is read once, in the constructor, from the creation context's storage, and
  written back in the trait's save method under its key. Never re-read storage mid-run.
- `value_type` is the project's type and must serialize and deserialize.
- With `default_on_error = true`, a corrupt or absent value falls back to the default; the load path never
  propagates the error or unwraps.

## Viewports

- Each `[[eframe.viewports]]` entry is one extra OS window, shown from the parent's painting code, with an id
  derived from the spec's `id` so it stays stable across runs.
- The viewport callback cannot borrow the app: move clones or shared handles into it.
- A `kind = "deferred"` entry is never shown as an immediate viewport: an immediate one runs inside the parent's
  pass, so a slow child stalls the root window.

## Entry point

- The run call owns the main thread until the last window closes, so it ends the entry point. Start what must
  outlive the UI before it and hand it in through the creator closure; cleanup during the session goes in the save
  or exit methods, not after the run call.
