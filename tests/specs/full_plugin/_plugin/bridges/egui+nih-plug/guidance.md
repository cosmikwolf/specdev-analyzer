# egui+nih-plug — implementation guidance

Rules an agent follows when it writes or edits the egui editor of a nih-plug plugin: parameter-to-widget bindings and
the editor host. No check verifies them yet; review does. The kind and purpose rules below are what a future unit
check will verify.

## The host

- The nih-plug egui adapter crate belongs to this pairing, not to either block. Inline this bridge whenever a plugin
  ships an egui editor, even with no bindings yet.
- The crate root re-exports the adapter's egui once. The adapter's egui uses the older panel API generation (panels
  shown into a `Context`); check its egui version when the adapter moves.
- The plugin's editor method creates one egui editor: its size from `[egui].default_size` (a small default when
  omitted), resizable when `[egui].resizable` is true.
- The editor closure runs per frame on the UI thread. It captures the shared parameter struct and any atomic meter
  cells — never the engine. Engine-derived values reach it as atomics written in `process`.

## Bindings

- Each `parameter_widgets[]` entry fills one widget into its region, in the region's layout. Its region must have
  `purpose = "parameter_controls"`.
- Widget kind must fit the parameter kind: `knob`, `slider` and `numeric_field` take float or int; `toggle` and
  `button` take bool; `dropdown` and `radio_group` take enum; `value_display` takes any. Refuse a mismatch.
- Every parameter change goes through the editor's setter as begin, set, end, so host automation sees it.
  `value_display` changes nothing.
- The adapter ships a parameter slider only. With no rotary primitive, `knob` uses the slider; toggles, dropdowns and
  radio groups are plain egui widgets wired through the setter. Enum widgets list variant names by index and rebuild
  values from the index.
- `button` is momentary: set true on click and false on the next frame; the audio side must handle the quick on and
  off.
- `range_hint` changes the value display only where it differs from the parameter's smoother curve.
- `label` places the label above, left, below or right of the widget, or omits it.

## Boundaries

- Crate type and manifest belong to the profile; the plugin trait, parameter struct and process callback belong to
  the nih-plug block; panels and regions belong to the egui block.
