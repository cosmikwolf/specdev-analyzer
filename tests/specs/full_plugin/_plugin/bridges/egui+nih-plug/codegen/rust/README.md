# egui+nih-plug — Rust codegen conventions

These conventions describe how `[bindings."egui+nih-plug".parameter_widgets]`
on a module spec maps to Rust source. They layer on top of the
audio-plugin profile + nih-plug block + egui block prose; concerns
specific to parameter↔widget binding emission live here.

The bridge fills in the parameter-widget calls inside
`purpose = "parameter_controls"` regions that the egui block's prose
leaves as placeholder. It is the only document that knows about both
the parameter's nih-plug type AND the egui rendering call — neither
source block knows about the other.

## Crate dependencies

This bridge contributes no new crates. It composes against:

- `nih_plug` (from the `nih-plug` block) — provides
  `widgets::ParamSlider`, `widgets::ParamButton`, `Setter`, parameter
  types.
- `nih_plug_egui` (from the `egui` block) — provides the editor
  closure context (`ctx`, `setter`, `state`).
- `egui_extras` (project-level dep) — provides knob primitives. Add
  this to the project's `architecture.toml` `[[external_deps]]` when
  the binding set uses `widget_kind = "knob"`. The bridge's prose
  references it but does NOT pull it in (project decision).

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

The 8 widget kinds (D50) map to egui calls as follows. Each entry
shows the canonical emission for one binding stanza inside a grid
region; vertical / horizontal layouts use the same widget call with
different label placement.

### `knob` — float / int parameter, rotary

```rust
ui.label(<param.name>);
ui.add(
    egui_extras::widgets::Knob::new(&mut self.params.<param.id>, setter)
        .with_range(/* derived from FloatRange */)
);
ui.end_row();
```

`egui_extras` is not on crates.io with a knob primitive in all
versions; if absent, fall back to `widgets::ParamSlider::for_param`
styled rotary (a vertical slider with smaller width is often a
sensible substitute). Document the dep in the project's
architecture.toml.

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
ui.add(widgets::ParamButton::for_param(&self.params.<param.id>, setter));
ui.end_row();
```

`ParamButton` renders as a press-to-toggle button (sticky on/off).

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
ui.add(widgets::ParamComboBox::for_param(&self.params.<param.id>, setter));
ui.end_row();
```

Where `ParamComboBox` is available; if not, fall back to a hand-rolled
`egui::ComboBox` driven by the `Enum` derive.

### `radio_group` — enum parameter, expanded

```rust
ui.label(<param.name>);
ui.horizontal(|ui| {
    let current = self.params.<param.id>.value();
    for variant in <EnumType>::variants() {
        if ui.radio_value(&mut current, variant, variant.to_string()).clicked() {
            setter.begin_set_parameter(&self.params.<param.id>);
            setter.set_parameter(&self.params.<param.id>, variant);
            setter.end_set_parameter(&self.params.<param.id>);
        }
    }
});
ui.end_row();
```

`<EnumType>` is the parameter's enum type from the nih-plug block's
emission (`#[derive(Enum)] pub enum FilterType { ... }`). The
`variants()` method comes from `nih_plug::params::enums::Enum`.

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
    self.params.<param.id>.unit_string(),
));
ui.end_row();
```

Read-only — no setter calls, no input handling. Useful for "current
value" readouts paired with a separate widget that drives the
parameter (e.g., a knob next to a numeric value display).

## Per-kind type constraints (discipline rules)

The bridge's emission code SHOULD reject mismatched kind/type pairs at
codegen time. Validate-time graph-check enforcement is bounded-deferred
per D50; the v1 emission discipline:

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
NIH-EGUI-003..009 errors when D50's bounded-deferral trigger fires.

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

Same rule as the egui block prose: the editor closure captures
`Arc<MyParams>` and any `Arc<Atomic*>` meter cells, NEVER the engine
struct. The `setter` parameter is an editor-thread argument provided
by `nih_plug_egui`; pass it through to all `setter.*` calls as shown
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

- **Crate-type emission.** The audio-plugin profile's prose owns
  Cargo.toml shape (cdylib + optional standalone bin per D45). This
  bridge contributes no Cargo.toml content.
- **Plugin trait skeleton.** The nih-plug block's prose owns the
  `impl Plugin for X` skeleton, the parameter struct (`#[derive(Params)]`),
  the export macros at crate root, and the `process()` callback shape.
  This bridge composes against the parameter struct but does not
  emit it.
- **Editor closure shell.** The egui block's prose owns
  `create_egui_editor` construction, panel emission ordering, and
  per-region `purpose`-driven scaffolds. This bridge fills in the
  `parameter_controls` region bodies, but does not own the closure
  itself.

This division of labor is what makes the bridge primitive earn its
keep architecturally — the bridge owns ONE concern (parameter ↔
widget wiring) that neither source block can express alone.
