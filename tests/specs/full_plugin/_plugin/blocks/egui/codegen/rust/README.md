# egui — Rust codegen conventions

These conventions describe how an `egui` block table on a module spec
maps to Rust source. They layer on top of the project's profile's
`module.conventions.md`; concerns specific to panel emission and
per-region scaffolding live here.

The block models **topology only** (EGUI-TOPOLOGY-ONLY): panels and
regions describe WHERE things go. It is **host-neutral**
(EGUI-NEEDS-A-HOST): it never opens a window and never creates an egui
context. Value→widget binding lives in a bridge against whichever unit
owns the values.

## Crate dependencies and the egui path

This block contributes **no crates**. egui arrives from the host, and
the host emits one line at the crate root:

```rust
pub use eframe::egui;          // when the `eframe` block is inlined
pub use nih_plug_egui::egui;   // when the `egui+nih-plug` bridge is inlined
```

Every file this block's prose emits opens with:

```rust
use crate::egui;
```

and refers to egui types through it — `egui::CentralPanel`,
`egui::Grid`, `egui::Context`, `egui::Ui`. **Never emit a direct
`egui::` path from an `egui` crate dependency, and never name a host's
crate here.** The alias is the whole mechanism: it keeps this block's
emission identical under every host, and it makes two hosts in one
crate fail at that single line instead of resolving two
semver-incompatible eguis whose types refuse to unify far downstream.

### When no host block is inlined

egui also runs under integrations this tree ships no block for — a
hand-rolled `winit` + `egui-wgpu` / `egui_glow` stack, an existing
application that already owns its window and event loop and wants egui
as a tools overlay, or headless use in tests. There the **project is
the host**.

In that case the project declares `egui` itself under
`architecture.toml` `[[external_deps]]`, which makes it a **direct**
dependency — so emitted code refers to plain **`egui::`** and there is
no alias at all. Do not emit `crate::egui`, and do not emit a
crate-root `pub use`: with no unit owning that line, it would have to
be hand-added to a spec-generated, lineage-recorded crate root, and the
next `/spec-gen` would delete it.

Everything else in this document is unchanged — only the path differs.
Note that not every integration crate re-exports egui (`eframe`,
`nih_plug_egui`, `egui-winit` and `bevy_egui` do; `egui-wgpu` and
`egui_glow` do not), which is a second reason the direct declaration is
the right move when no host block is in play.

**Do NOT** add `eframe`, `nih_plug_egui`, or `egui_extras` from this
block. The first two are hosts; `egui_extras` is the project's choice
and lands in its architecture `[[external_deps]]`.

## Two panel API generations — determine which one applies FIRST

egui changed its panel API, and the two shipped hosts sit one on each
side of the change. **Before emitting any panel code, determine which
form applies**, then use that form throughout. Everything else in this
document — regions, layouts, purposes — is identical in both.

Determine it by feature, not by memory:

| If the host's egui has… | Use | Which host |
|---|---|---|
| `egui::Panel` and `show` taking a `&mut Ui` | **modern form** | `eframe` (egui 0.36) |
| `egui::SidePanel` / `egui::TopBottomPanel` and `show` taking a `&Context` | **legacy form** | `nih_plug_egui` (egui 0.31) |

The version history behind that, for orientation only — the feature
check above is the rule:

| egui | Panel types | `show` takes | Sizing |
|---|---|---|---|
| ≤ 0.33 | `SidePanel` / `TopBottomPanel` / `CentralPanel` | `&Context` | `default_width` / `default_height` |
| 0.34 | unified `Panel` + `CentralPanel` | `&Context` | `default_size` |
| ≥ 0.35 | unified `Panel` + `CentralPanel` | `&mut Ui` | `default_size` |

Both forms below are compiled in this block's reference crate
(`panels` and `panels_legacy`), so neither is written from memory.

## What this block emits

One function per module carrying `[egui]`, holding every panel in
declaration order. **The parameter type is the first thing the form
decides**, because it is what the host hands its paint callback:

```rust
use crate::egui;

/// Modern form — the host hands its paint callback a `Ui`.
pub fn draw(ui: &mut egui::Ui) {
    // panels emit here, in the order declared in the spec
}

/// Legacy form — the host hands its paint callback a `Context`.
pub fn draw(ctx: &egui::Context) {
    // panels emit here, in the order declared in the spec
}
```

The host's prose owns the callback that calls `draw` — an
`eframe::App::ui` body, a plugin editor's update closure, whatever the
host provides. This block is silent on how that callback comes to
exist.

**A bridge may extend this signature.** Widget stanzas composed into
`parameter_controls` regions need their values in scope, so a bridge
that fills those regions specifies the extra parameters in its own
prose (the `egui+nih-plug` bridge adds a `ParamSetter` and the params
handle). The panel and region structure inside the body is unchanged
by that extension.

## Panel emission

Panels emit in the order they appear in `[[egui.panels]]`. Side / top /
bottom panels must come BEFORE the central panel, because each panel
claims its strip and the central panel fills what's left. Codegen
reorders if needed but emits in spec order otherwise.

### Modern form (`egui::Panel`, `show` takes a `&mut Ui`)

| `kind`        | egui type                          | Sizing            |
|---------------|------------------------------------|-------------------|
| `central`     | `egui::CentralPanel::default()`     | —                 |
| `side_left`   | `egui::Panel::left(<id>)`           | `.default_size(<width>)`  |
| `side_right`  | `egui::Panel::right(<id>)`          | `.default_size(<width>)`  |
| `top`         | `egui::Panel::top(<id>)`            | `.default_size(<height>)` |
| `bottom`      | `egui::Panel::bottom(<id>)`         | `.default_size(<height>)` |

```rust
pub fn draw(ui: &mut egui::Ui) {
    egui::Panel::top("header")
        .default_size(40.0)
        .show(ui, |ui| {
            // regions in the "header" panel emit here
        });

    egui::CentralPanel::default().show(ui, |ui| {
        // regions in the "main" panel emit here
    });
}
```

### Legacy form (`SidePanel` / `TopBottomPanel`, `show` takes a `&Context`)

| `kind`        | egui type                                | Sizing                     |
|---------------|------------------------------------------|----------------------------|
| `central`     | `egui::CentralPanel::default()`           | —                          |
| `side_left`   | `egui::SidePanel::left(<id>)`             | `.default_width(<width>)`  |
| `side_right`  | `egui::SidePanel::right(<id>)`            | `.default_width(<width>)`  |
| `top`         | `egui::TopBottomPanel::top(<id>)`         | `.default_height(<height>)`|
| `bottom`      | `egui::TopBottomPanel::bottom(<id>)`      | `.default_height(<height>)`|

```rust
pub fn draw(ctx: &egui::Context) {
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

Per-panel attributes — the spec fields are the same in both forms, only
the setter differs (see the sizing column of the form's table above):

- `width` (only for `side_left` / `side_right`)
- `height` (only for `top` / `bottom`)

Codegen rejects `width` on non-side panels and `height` on non-top/bottom
panels with a warning comment (the schema allows the fields on any
panel kind, but the egui types only honor them for the matching
positions).

`[egui].default_size` and `[egui].resizable` are consumed by the
**host**, not here — a host with no size of its own reads them, and a
host that declares its own window geometry ignores them. This block
emits nothing for either field.

## Per-region scaffolding by purpose

Inside each panel's `.show(…, |ui| { ... })` closure, regions emit in
declaration order. **This section is identical in both panel forms** —
the closure hands you a `&mut Ui` either way, and every call below is
on that `Ui`. The region's `purpose` drives the scaffold:

### `parameter_controls`

A bridge owns this. The egui block emits a placeholder marker; a
bridge's prose composes widget calls into the region body. Worked
example for a region with `layout = "grid"`, `columns = 2`:

```rust
egui::Grid::new("param_grid").num_columns(2).show(ui, |ui| {
    // BRIDGE FILL: value-bound widgets for region "param_grid" emit here.
});
```

For `layout = "vertical"` (default):

```rust
ui.vertical(|ui| {
    // BRIDGE FILL: value-bound widgets for region "<id>" emit here.
});
```

For `layout = "horizontal"`:

```rust
ui.horizontal(|ui| {
    // BRIDGE FILL: value-bound widgets for region "<id>" emit here.
});
```

When `layout` is omitted, codegen defaults to `vertical`. When
`columns` is set on a non-grid layout, codegen emits a warning
comment and ignores it.

**With no bridge inlined**, the marker becomes a hand-authored body —
the region is still structurally declared, it just has no unit
supplying its widgets.

### `metering`

The region displays values produced somewhere else, typically another
thread publishing through `Arc<Atomic*>` cells. Codegen emits the read
boilerplate but leaves the visualization body for hand-authoring
(rendering depends on cosmetic preference: bar / numeric / LED-style).

The published cell reaches the emitted code as a parameter — a
`metering` region is one of the cases that extends the signature:

```rust
pub fn metering(ui: &mut egui::Ui, level: &std::sync::atomic::AtomicU32) {
    ui.vertical(|ui| {
        // Read a value published by the producing thread. `level` is a
        // shared cell the producer writes and this side only reads.
        let level = f32::from_bits(level.load(std::sync::atomic::Ordering::Relaxed));
        // TODO: render — bar / LED / numeric per author preference.
        // Example: ui.label(format!("{level:.2}"));
        let _ = level;
        ui.label("METERING (hand-authored)");
    });
}
```

Emission of the **producing** side is not this block's concern; the
project's profile prose owns it, including any thread-model rules about
where the value may be written from.

### `preset_selector`

Standard saved-configuration picker: combo box + prev/next buttons +
save/load. Codegen emits the layout; the configuration list and its
persistence are hand-authored or come from a separate facility (out of
scope for v1).

```rust
ui.horizontal(|ui| {
    if ui.button("◀").clicked() {
        // TODO: previous entry
    }
    egui::ComboBox::from_label("Preset")
        .selected_text(/* current entry name */)
        .show_ui(ui, |ui| {
            // TODO: enumerate entries
        });
    if ui.button("▶").clicked() {
        // TODO: next entry
    }
    if ui.button("Save").clicked() {
        // TODO: save current
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
    // plots / spectrum analyzers / curve editors / graph views /
    // XY pads / waveform displays.
    //
    // Read-only state should arrive through shared cells or channels
    // from whatever produces it, never by holding a reference to a
    // producer that lives on another thread.
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

## Interaction with the host, the profile, and bridges

This block describes WHERE things go. Three neighbours fill in the
rest:

- **The host** (the `eframe` block, or the `egui+nih-plug` bridge)
  owns the crate-root `pub use …::egui;` alias, the window or editor
  the interface lives in, and the paint callback that calls `draw`.
- **The project's profile** owns module and struct shape, Cargo.toml
  shape, and any thread-model rules for the values regions display.
- **A bridge** owns value→widget binding — the placeholder markers
  this block leaves in `parameter_controls` regions.

This block is silent on widget rendering, on the shape of the values
being rendered, on window and event-loop concerns, and on Cargo.toml
content. Read those documents for those concerns.

**Do NOT** emit widget calls bound to declared values in this block's
prose — that's a bridge's responsibility. **Do NOT** emit window,
editor, or event-loop setup here — that's the host's.
