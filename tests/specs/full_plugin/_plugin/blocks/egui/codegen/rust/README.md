# egui — Rust codegen conventions

These conventions describe how an `egui` block table on a module spec
maps to Rust source. They layer on top of the audio-plugin profile's
`module.conventions.md`; concerns specific to the editor frame, panel
emission, and per-region scaffolding live here.

The block targets [egui](https://github.com/emilk/egui) via
[nih_plug_egui](https://github.com/robbert-vdh/nih-plug/tree/master/nih_plug_egui)
— nih-plug's egui adapter. The block models **topology only** (D47):
panels and regions describe WHERE things go; parameter→widget bindings
live in the `egui+nih-plug` bridge.

## Crate dependencies

This block contributes one crate:

```toml
nih_plug_egui = { git = "https://github.com/robbert-vdh/nih-plug.git", rev = "<pin>" }
```

`nih_plug_egui` re-exports the egui types this block needs
(`egui::Context`, `egui::Ui`, panel constructors, etc.) — no separate
`egui` dep is required. Pin the same nih-plug rev as the `nih-plug`
block; they ship from the same repo.

**Do NOT** add `eframe` or `egui_extras` from this block — those are
the project's choice (`egui_extras` is commonly added when the bridge
needs knob primitives; project lands it in their architecture
`[[external_deps]]`).

## Editor closure shape

Every plugin shell module that carries `[egui]` emits one
`create_egui_editor` call wired into the plugin's `editor()` method.
The editor closure runs per frame on the UI thread; it captures
`Arc<MyParams>` and any `Arc<Atomic*>` meter cells, but NEVER captures
the engine struct (the engine lives behind `Arc<Mutex<...>>` or in
audio-thread-owned state — passing it through the editor is a thread-
safety violation).

Skeleton emitted into the plugin shell module:

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
            // update_fn — runs per frame
            // panel emission goes here, in the order declared in spec
        },
    )
}
```

The `editor()` method is implemented on the plugin (`impl Plugin for
MyPlugin { fn editor(...) -> ... }`) — not in this module's body. The
audio-plugin profile's prose covers the `impl Plugin` skeleton; this
block's prose fills in the editor closure body.

When `[egui].default_size` is omitted, codegen falls back to
`EguiState::from_size(400, 300)` (egui's conventional small editor).

When `[egui].resizable = true`, codegen calls
`editor_state.set_resizable(true)` after construction.

## Panel emission

Panels emit in the order they appear in `[[egui.panels]]`. egui has a
strict ordering rule for the frame builder: side / top / bottom panels
must come BEFORE the central panel, because each panel removes its
strip from the available area and the central panel fills what's
left. Codegen reorders if needed but emits in spec order otherwise:

| `kind`        | egui type                                |
|---------------|------------------------------------------|
| `central`     | `egui::CentralPanel::default()`           |
| `side_left`   | `egui::SidePanel::left(<id>)`             |
| `side_right`  | `egui::SidePanel::right(<id>)`            |
| `top`         | `egui::TopBottomPanel::top(<id>)`         |
| `bottom`      | `egui::TopBottomPanel::bottom(<id>)`      |

Worked example for a header + main editor:

```rust
move |ctx, setter, state| {
    egui::TopBottomPanel::top("header")
        .default_height(40.0)
        .show(ctx, |ui| {
            // regions in the "header" panel emit here
        });

    egui::CentralPanel::default().show(ctx, |ui| {
        // regions in the "main" panel emit here
    });
}
```

Per-panel attributes:
- `width` (only for `side_left` / `side_right`) → `.default_width(<width>)`
- `height` (only for `top` / `bottom`) → `.default_height(<height>)`

Codegen rejects `width` on non-side panels and `height` on non-top/bottom
panels with a warning comment (the schema allows the fields on any
panel kind, but the egui types only honor them for the matching
positions).

## Per-region scaffolding by purpose

Inside each panel's `.show(ctx, |ui| { ... })` closure, regions emit
in declaration order. The region's `purpose` drives the scaffold:

### `parameter_controls`

The bridge owns this. The egui block emits a placeholder marker; the
`egui+nih-plug` bridge's prose composes parameter-widget calls into
the region body. Worked example for a region with `layout = "grid"`,
`columns = 2`:

```rust
egui::Grid::new("param_grid").num_columns(2).show(ui, |ui| {
    // BRIDGE FILL: parameter_widgets[] entries with region = "param_grid"
    // emit here. Each becomes:
    //   ui.label(<param.name>);
    //   <widget_call>;
    //   ui.end_row();
});
```

For `layout = "vertical"` (default):

```rust
ui.vertical(|ui| {
    // BRIDGE FILL: parameter_widgets[] entries
});
```

For `layout = "horizontal"`:

```rust
ui.horizontal(|ui| {
    // BRIDGE FILL: parameter_widgets[] entries
});
```

When `layout` is omitted, codegen defaults to `vertical`. When
`columns` is set on a non-grid layout, codegen emits a warning
comment and ignores it.

### `metering`

The region reads from audio-thread-published state via `Arc<Atomic*>`
cells. Codegen emits the read boilerplate but leaves the visualization
body for hand-authoring (proper meter rendering depends on cosmetic
preference: bar / numeric / LED-style).

```rust
ui.vertical(|ui| {
    // Read meter values published by the audio thread.
    // Plugin shell module declares these as Arc<AtomicF32> fields:
    //
    //   pub struct MyPlugin {
    //       params: Arc<MyPluginParams>,
    //       output_level: Arc<AtomicF32>,  // populated in process()
    //   }
    //
    let level = self.output_level.load(std::sync::atomic::Ordering::Relaxed);
    // TODO: render meter — bar / LED / numeric per author preference.
    // Example: ui.label(format!("{:.1} dB", 20.0 * level.log10()));
    ui.label("METERING (hand-authored)");
});
```

The audio-plugin profile prose covers the audio-thread side
(allocate `Arc<AtomicF32>` in `initialize()`, write in `process()`).

### `preset_selector`

Standard preset-picker pattern: combo box + prev/next buttons +
save/load. Codegen emits the layout; preset list / persistence is
hand-authored or comes from a separate preset-management facility
(out of scope for v1).

```rust
ui.horizontal(|ui| {
    if ui.button("◀").clicked() {
        // TODO: previous preset
    }
    egui::ComboBox::from_label("Preset")
        .selected_text(/* current preset name */)
        .show_ui(ui, |ui| {
            // TODO: enumerate presets
        });
    if ui.button("▶").clicked() {
        // TODO: next preset
    }
    if ui.button("Save").clicked() {
        // TODO: save preset
    }
});
```

### `free_form`

Hand-authored region body. Codegen emits an explicit marker and leaves
the body open:

```rust
ui.vertical(|ui| {
    // FREE-FORM REGION: <region.id>
    // Hand-authored body. Author owns rendering — typical uses:
    // oscilloscope / spectrum analyzer / EQ curve / modulation matrix /
    // waveform editor / XY pad.
    //
    // Read-only state should come from `Arc<Atomic*>` or lock-free
    // ring buffers published by the audio thread, never the engine
    // struct itself.
});
```

### `label_block`

Static text region — section headers, brief instructions, signal-flow
diagrams. Codegen emits `ui.label(...)` placeholders; the spec author
fills in the strings (a future schema extension could add a `text:
[strings]` field, but v1 keeps it open):

```rust
ui.vertical(|ui| {
    // LABEL BLOCK: <region.id>
    ui.label("Section Header");
    ui.label("Sub-text or instructions");
});
```

## Layout direction emission

`layout` wraps the region body in the corresponding egui layout call:

| `layout`     | Wrapper                                                  |
|--------------|----------------------------------------------------------|
| (omitted) / `vertical` | `ui.vertical(|ui| { ... })`                          |
| `horizontal`           | `ui.horizontal(|ui| { ... })`                        |
| `grid`                 | `egui::Grid::new(id).num_columns(<columns>).show(ui, |ui| { ... })` |

For `grid`, `columns` must be set; codegen rejects unset columns on
grid layouts with a warning.

## State ownership rule

The editor closure captures **`Arc<MyParams>` and any `Arc<Atomic*>`
meter cells, NEVER the engine struct**. Three reasons:

1. The editor runs on the UI thread; the engine runs on the audio
   thread. Sharing engine state requires `Arc<Mutex<...>>` and locking
   on the audio path, which violates the realtime contract (no locks).
2. egui closures are `'static` — capturing references doesn't compile
   without `Arc`-wrapping. Wrapping the engine in `Arc<Mutex<...>>`
   means the audio path locks every block.
3. The decoupling matches nih-plug's design — params + meters are the
   thread-safe surface; everything else is per-thread.

When the editor needs to display engine-derived state (output level,
voice count active), the plugin shell module publishes via
`Arc<AtomicF32>` / `Arc<AtomicUsize>` updated from `process()`. The
editor reads them with `.load(Relaxed)`. Codegen prose for `metering`
regions documents this pattern; emission of the audio-thread side
lives in the user's `process()` body.

## Interaction with audio-plugin + nih-plug conventions

This block describes WHERE controls go. Two siblings fill in WHAT
goes there:

- **`nih-plug` block** owns parameter declaration (`[[nih_plug.parameters]]`)
  and the plugin trait skeleton (`impl Plugin for MyPlugin`). The
  `Arc<MyParams>` the editor closure captures comes from the
  `#[derive(Params)]` struct that block's prose emits.
- **`egui+nih-plug` bridge** owns parameter↔widget bindings
  (`[[bindings."egui+nih-plug".parameter_widgets]]`). The bridge's
  prose composes per-binding widget calls into `parameter_controls`
  regions (the placeholder this block leaves open).

This block is silent on widget rendering, on parameter struct shape,
on plugin-trait constants, on Cargo.toml shape (the audio-plugin
profile prose owns crate-type emission). Read those documents for
those concerns.

**Do NOT** emit parameter-widget calls in this block's prose — that's
the bridge's responsibility. **Do NOT** emit `impl Plugin` content
here — that's the nih-plug block's responsibility.
