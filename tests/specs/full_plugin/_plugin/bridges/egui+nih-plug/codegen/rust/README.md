# egui+nih-plug — Rust codegen conventions

These conventions describe how `[bindings."egui+nih-plug".parameter_widgets]`
on a module spec maps to Rust source. They layer on top of the
project's profile + nih-plug block + egui block prose; concerns
specific to parameter↔widget binding emission live here.

The bridge fills in the parameter-widget calls inside
`purpose = "parameter_controls"` regions that the egui block's prose
leaves as placeholder. It is the only document that knows about both
the parameter's nih-plug type AND the egui rendering call — neither
source block knows about the other.

## Crate dependencies

This bridge contributes **one crate**, `nih_plug_egui` — the join
product of this pairing. It is the adapter that lets a nih-plug plugin
host an egui interface, so it exists only because both endpoints are in
play, and neither endpoint block may declare it.

It composes against:

- `nih_plug` (from the `nih-plug` block) — provides `ParamSetter`,
  parameter types (`FloatParam`/`IntParam`/`BoolParam`/`EnumParam`), and
  the `Enum` trait (`variants()` / `from_index()` / `to_index()`).
- `nih_plug_egui` (this bridge's own contribution) — provides
  `create_egui_editor`, `EguiState`, the editor closure context (`ctx`,
  `setter`, `state`), the re-exported egui, and
  `widgets::ParamSlider` (the only parameter widget it ships: there is
  **no** `ParamButton` or `ParamComboBox`, so `toggle`/`dropdown` use
  plain egui widgets + the setter — see those kinds below).

## Host binding — this bridge is the egui host for a plugin

The `egui` block models topology and contributes no crates; egui always
arrives from a host. For a plugin, **this bridge is that host**. Three
obligations follow, all of them this bridge's.

### 1. The crate-root egui alias

Emit exactly one line at the crate root:

```rust
pub use nih_plug_egui::egui;
```

The egui block's emitted topology code refers to `crate::egui` and
resolves through this. Emit it once per crate; a second host in the
same crate would collide here, which is the intended failure.

### 2. Which panel API generation applies

`nih_plug_egui` pins **egui 0.31**, which predates egui's panel-API
change. So the egui block's **legacy form** applies throughout this
stack: `SidePanel` / `TopBottomPanel` / `CentralPanel`, `show` taking a
`&Context`, sizes via `default_width` / `default_height`.

Read the panel table from the egui block's prose — it is the single
source for panel emission and carries both forms. This section only
states which one to pick. When `nih_plug_egui` moves to egui 0.34 or
newer, this paragraph is what changes.

### 3. The editor closure

Every plugin shell module that carries `[egui]` emits one
`create_egui_editor` call wired into the plugin's `editor()` method.
The editor closure runs per frame on the UI thread; it captures
`Arc<MyParams>` and any `Arc<Atomic*>` meter cells, but NEVER captures
the engine struct (the engine lives behind `Arc<Mutex<...>>` or in
audio-thread-owned state — passing it through the editor is a thread-
safety violation).

```rust
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, EguiState};
use std::sync::Arc;

#[derive(Default)]
struct EditorState {
    // editor-thread state goes here (cached UI values, drag-in-progress
    // flags, preset-picker open/closed). NEVER engine state.
}

fn editor() -> Option<Box<dyn Editor>> {
    let editor_state = EguiState::from_size(<default_size.width>, <default_size.height>);
    let params = self.params.clone();
    create_egui_editor(
        editor_state,
        EditorState::default(),
        |_ctx, _state| {},  // build_fn — runs once
        move |ctx, setter, state| {
            // update_fn — runs per frame. The egui block's panel
            // emission goes here, in the legacy form, with `ctx`.
        },
    )
}
```

The `editor()` method is implemented on the plugin (`impl Plugin for
MyPlugin { fn editor(...) -> ... }`) — not in the module's body. The
project's profile's prose covers the `impl Plugin` skeleton; this
bridge's prose fills in the editor closure body.

`EguiState::from_size` consumes `[egui].default_size`; when it is
omitted, fall back to `EguiState::from_size(400, 300)` (egui's
conventional small editor). When `[egui].resizable = true`, call
`editor_state.set_resizable(true)` after construction. Those two fields
live on the egui block's table but are consumed here, because the host
is what has a window to size.

## Editor closure plumbing

The bridge's emission lives inside the editor closure that the egui
block's prose scaffolds. Per-binding widget calls go inside the
appropriate region's body (which the egui block left as a placeholder
with the marker comment `// BRIDGE FILL: ...`).

For a region with `layout = "grid"` and `columns = 2`, the egui
block's scaffold is:

```rust
egui::Grid::new("param_grid").num_columns(2).show(ui, |ui| {
    // BRIDGE FILL: parameter_widgets[] entries with region = "param_grid"
});
```

The bridge replaces that comment with one stanza per binding. Stanza
shape per-grid:

```rust
ui.label(<param.name>);  // when label = "above" / "left" (default for grid)
<widget_call>;
ui.end_row();
```

For `layout = "vertical"` regions:

```rust
ui.vertical(|ui| {
    // per-binding stanzas — labels above by default
});
```

For `layout = "horizontal"` regions: similar but `ui.horizontal`,
labels to the left by default.

## Per-kind widget emission

The 8 widget kinds (WIDGET-KIND-V1) map to egui calls as follows. Each entry
shows the canonical emission for one binding stanza inside a grid
region; vertical / horizontal layouts use the same widget call with
different label placement.

### `knob` — float / int parameter, rotary

```rust
ui.label(<param.name>);
// No rotary knob primitive exists in the pinned stack: emit a ParamSlider
// fallback (style as a narrow vertical slider if a rotary look is wanted).
ui.add(widgets::ParamSlider::for_param(&self.params.<param.id>, setter));
ui.end_row();
```

There is no rotary knob primitive in the pinned `nih_plug_egui` /
`egui_extras`, so `knob` renders as a `widgets::ParamSlider` (the same
automation-aware slider as `slider`, optionally styled narrow/vertical).
If a future `egui_extras` adds a knob widget, swap the call and document
the dep in the project's architecture.toml.

### `slider` — float / int parameter, linear

```rust
ui.label(<param.name>);
ui.add(widgets::ParamSlider::for_param(&self.params.<param.id>, setter));
ui.end_row();
```

`widgets::ParamSlider` is nih-plug's standard slider, automation- and
host-aware. Emits horizontally; for a vertical slider, use
`.with_orientation(SliderOrientation::Vertical)` (not exposed via
`for_param` shorthand; user adjusts manually if needed).

### `toggle` — bool parameter, sticky

```rust
ui.label(<param.name>);
let mut value = self.params.<param.id>.value();
if ui.checkbox(&mut value, <param.name>).changed() {
    setter.begin_set_parameter(&self.params.<param.id>);
    setter.set_parameter(&self.params.<param.id>, value);
    setter.end_set_parameter(&self.params.<param.id>);
}
ui.end_row();
```

`nih_plug_egui` ships no `ParamButton`, so a sticky bool toggle is a
plain egui `checkbox` wired through the `setter` (begin/set/end) so host
automation observes the change. (`egui::Checkbox`/`toggle_value` are
alternatives with the same setter plumbing.)

### `button` — bool parameter, momentary

```rust
ui.label(<param.name>);
let response = ui.button(<param.name>);
if response.clicked() {
    setter.begin_set_parameter(&self.params.<param.id>);
    setter.set_parameter(&self.params.<param.id>, true);
    setter.end_set_parameter(&self.params.<param.id>);
    // momentary semantics: schedule an end-frame reset to false.
    // The plugin author may track a per-frame flag in editor state
    // and reset on the next frame's update.
}
ui.end_row();
```

Momentary buttons are subtler than toggle — nih-plug doesn't have a
first-class momentary parameter type. The pattern: set true on
`clicked()`, reset to false on the next frame. The
`process()` callback observes both edges (host receives the on event
+ the reset event in sequence). Document this in the binding's
context note: `button` works for trigger-style actions
(panic, randomize, reset, tap-tempo) but the audio-thread side must
handle the rapid-fire on/off pattern.

### `dropdown` — enum parameter, collapsed

```rust
ui.label(<param.name>);
let mut current = self.params.<param.id>.value();
let names = <EnumType>::variants();
egui::ComboBox::from_label(<param.name>)
    .selected_text(names[current.to_index()])
    .show_ui(ui, |ui| {
        for (i, name) in names.iter().enumerate() {
            let variant = <EnumType>::from_index(i);
            if ui.selectable_value(&mut current, variant, *name).clicked() {
                setter.begin_set_parameter(&self.params.<param.id>);
                setter.set_parameter(&self.params.<param.id>, current);
                setter.end_set_parameter(&self.params.<param.id>);
            }
        }
    });
ui.end_row();
```

`nih_plug_egui` ships no `ParamComboBox`, so a collapsed enum selector is
a hand-rolled `egui::ComboBox` driven by the `#[derive(Enum)]` type.
`Enum::variants()` yields the variant *names* (`&[&str]`); reconstruct
each value with `Enum::from_index(i)` and map the current value to its
label via `to_index()`.

### `radio_group` — enum parameter, expanded

```rust
ui.label(<param.name>);
let mut current = self.params.<param.id>.value();
let names = <EnumType>::variants();
ui.horizontal(|ui| {
    for (i, name) in names.iter().enumerate() {
        let variant = <EnumType>::from_index(i);
        if ui.radio_value(&mut current, variant, *name).clicked() {
            setter.begin_set_parameter(&self.params.<param.id>);
            setter.set_parameter(&self.params.<param.id>, variant);
            setter.end_set_parameter(&self.params.<param.id>);
        }
    }
});
ui.end_row();
```

`<EnumType>` is the parameter's enum type from the nih-plug block's
emission (`#[derive(Enum)] pub enum FilterType { ... }`).
`Enum::variants()` (from `nih_plug::params::enums::Enum`) yields the
variant *names* as `&[&str]`, NOT the enum values — iterate by index and
reconstruct each value with `Enum::from_index(i)`. Use `let mut current`
(`radio_value` takes `&mut`).

### `numeric_field` — float / int parameter, typed entry

```rust
ui.label(<param.name>);
let mut value = self.params.<param.id>.value();
let response = ui.add(
    egui::DragValue::new(&mut value)
        .clamp_range(<param.min>..=<param.max>)
);
if response.changed() {
    setter.begin_set_parameter(&self.params.<param.id>);
    setter.set_parameter(&self.params.<param.id>, value);
    setter.end_set_parameter(&self.params.<param.id>);
}
ui.end_row();
```

`DragValue` accepts both typed input and click-and-drag adjustment.
Wire through `setter.begin_set_parameter` /
`setter.set_parameter` / `setter.end_set_parameter` so host
automation picks up the change.

### `value_display` — any parameter, read-only

```rust
ui.label(<param.name>);
ui.label(format!(
    "{}{}",
    self.params.<param.id>.value(),
    self.params.<param.id>.unit(),
));
ui.end_row();
```

Read-only — no setter calls, no input handling. Useful for "current
value" readouts paired with a separate widget that drives the
parameter (e.g., a knob next to a numeric value display).

## Per-kind type constraints (discipline rules)

The bridge's emission code SHOULD reject mismatched kind/type pairs at
codegen time. Validate-time graph-check enforcement is bounded-deferred
per WIDGET-KIND-V1; the v1 emission discipline:

| widget_kind     | required parameter.kind  | violation behavior at codegen |
|-----------------|--------------------------|-------------------------------|
| `knob`          | `float` or `int`         | Emit error; refuse to compose. |
| `slider`        | `float` or `int`         | Emit error; refuse to compose. |
| `toggle`        | `bool`                   | Emit error; refuse to compose. |
| `button`        | `bool`                   | Emit error; refuse to compose. |
| `dropdown`      | `enum`                   | Emit error; refuse to compose. |
| `radio_group`   | `enum`                   | Emit error; refuse to compose. |
| `numeric_field` | `float` or `int`         | Emit error; refuse to compose. |
| `value_display` | any                      | (no constraint)                |

Region-purpose constraint: each `parameter_widgets[]` entry's region
SHOULD have `purpose = "parameter_controls"`. Bindings into other
purposes (metering, free_form, label_block, preset_selector) emit a
codegen-time warning and are skipped. These will become validate-time
NIH-EGUI-003..009 errors when WIDGET-KIND-V1's bounded-deferral trigger fires.

## Range hint emission

The optional `range_hint` field on a binding overrides the parameter's
smoother-derived display formatting:

| `range_hint`    | Effect                                                          |
|-----------------|-----------------------------------------------------------------|
| (omitted)       | Inherit from `parameter.smoother` (linear if smoother = none/linear; logarithmic / exponential if smoother matches). |
| `linear`        | Force linear value display.                                     |
| `logarithmic`   | Force logarithmic value display (knobs / sliders show log scale). |
| `exponential`   | Force exponential value display.                                |

Codegen attaches this via `.with_value_to_string()` /
`.with_string_to_value()` on the parameter — only when the
range_hint diverges from the parameter's smoother (otherwise the
default formatting is correct).

## Label position emission

The optional `label` field positions the `ui.label` call relative to
the widget:

| `label`          | Emission                                                              |
|------------------|-----------------------------------------------------------------------|
| (omitted) / `above` | `ui.label(<name>); <widget>;` (default for vertical layouts)        |
| `left`           | `ui.label(<name>); <widget>;` inside a horizontal sub-layout (default for grid / horizontal layouts) |
| `below`          | `<widget>; ui.label(<name>);`                                          |
| `right`          | `<widget>; ui.label(<name>);` inside a horizontal sub-layout           |
| `none`           | Omit `ui.label` entirely (caller may have a separate label_block region) |

Codegen warns when `label = "right"` is used in a vertical layout
(visual ambiguity); the warning is informational, not an error.

## State ownership rule

The editor closure captures `Arc<MyParams>` and any `Arc<Atomic*>`
meter cells, NEVER the engine struct. Three reasons:

1. The editor runs on the UI thread; the engine runs on the audio
   thread. Sharing engine state requires `Arc<Mutex<...>>` and locking
   on the audio path, which violates the realtime contract (no locks).
2. egui closures are `'static` — capturing references doesn't compile
   without `Arc`-wrapping. Wrapping the engine in `Arc<Mutex<...>>`
   means the audio path locks every block.
3. The decoupling matches nih-plug's design — params + meters are the
   thread-safe surface; everything else is per-thread.

When the editor needs engine-derived state (output level, active voice
count), the plugin shell module publishes it via `Arc<AtomicF32>` /
`Arc<AtomicUsize>` updated from `process()`, and the editor reads with
`.load(Relaxed)`. The egui block's `metering` region scaffold documents
the read side; emission of the audio-thread side lives in the user's
`process()` body.

The `setter` parameter is an editor-thread argument provided by
`nih_plug_egui`; pass it through to all `setter.*` calls as shown
above.

The bridge's emission must:
- Use `&self.params.<id>` to read parameter values (where `params` is
  the captured `Arc<MyParams>`).
- Pass `setter` (received from the editor closure) into every
  parameter-modifying call.
- NEVER hold a `Mutex<Engine>` or any non-`Arc<Atomic*>` engine
  reference in the editor closure.

## What this bridge does NOT do

Three concerns this bridge is silent on (handled elsewhere):

- **Crate-type emission.** The project's profile's prose owns
  Cargo.toml shape (cdylib + optional standalone bin). This
  bridge contributes no Cargo.toml content beyond the one crate in its
  `deps.yaml`.
- **Plugin trait skeleton.** The nih-plug block's prose owns the
  `impl Plugin for X` skeleton, the parameter struct (`#[derive(Params)]`),
  the export macros at crate root, and the `process()` callback shape.
  This bridge composes against the parameter struct but does not
  emit it.
- **Panel and region emission.** The egui block's prose owns panel
  ordering, the panel-kind tables for both API generations, and the
  per-region `purpose`-driven scaffolds. This bridge says which
  generation applies and fills in the `parameter_controls` region
  bodies; it does not restate the panel code.

This division of labour is what makes the bridge primitive earn its
keep architecturally. The bridge owns two concerns neither source block
can express alone: parameter ↔ widget wiring, and the host binding —
the join-product crate plus the editor closure that gives the topology
an egui context to draw into.

**Inline this bridge whenever a nih-plug plugin ships an egui editor**,
even with no `parameter_widgets` entries yet. The bindings table may be
empty; the host binding above is what the stack needs to compile.
