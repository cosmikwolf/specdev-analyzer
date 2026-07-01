# egui block

Editor-topology block targeting [egui](https://github.com/emilk/egui)
via [nih_plug_egui](https://github.com/robbert-vdh/nih-plug/tree/master/nih_plug_egui).
Contributes a namespaced `[egui]` table on `Module` describing
**editor topology** — panels and regions — for plugin shells that
ship a custom UI.

## When to use this block

Inline `egui` when your project:

- Has a deterministic editor structure — known panels, known regions
  inside them, known purposes (parameter controls, metering, preset
  selector). egui suits both immediate-mode UIs and dashboard-style
  editors.
- Ships that editor as a plugin UI. The block targets egui via
  `nih_plug_egui` today, so its current home is an audio-plugin shell
  module. (Pair with the `nih-plug` block + `egui+nih-plug` bridge for
  the full plugin-shell + parameter-widget surface.)

Don't use this block if:

- Your editor is hand-rolled with no panel/region structure (a single
  `CentralPanel` with hand-coded layout). The block adds no value
  there; an `[egui] enabled = true` flag would be lighter, but the
  sharp-test rule says blocks must contribute concrete shapes.
- You're targeting a different UI toolkit (iced, vizia, nih-plug-vizia,
  raw skia). Those would land as separate blocks; the topology
  vocabulary would likely differ enough to justify it.

## Why topology only (EGUI-TOPOLOGY-ONLY)

The egui block models topology — WHERE controls live — but NOT WHICH
widgets render WHICH parameters. That cross-cutting concern lives in
the `egui+nih-plug` bridge. Three reasons:

1. **egui is a UI library, not a widget catalog.** Panel + region
   layout is the right abstraction for a block. Widget content is
   too fine-grained and varies too widely (knob style, label
   placement, custom drawing) to schema usefully without ballooning
   the surface.
2. **Parameter→widget binding is exactly what bridges are for.** It's
   a wiring between nih-plug (owner of `Parameter`) and egui (owner
   of widget rendering). A bridge primitive is the right home for
   cross-cutting wiring.
3. **`free_form` regions accommodate custom drawing** without forcing
   it into the spec. Oscilloscopes / modulation visualizers /
   spectrum analyzers / XY pads / envelope editors are declared
   structurally — "this region exists, content is hand-authored" —
   but Claude can still emit a coherent scaffold around them.

## What this block contributes

Schema additions on `Module`:

```toml
[egui]
default_size = { width = 480, height = 320 }
resizable    = true

[[egui.panels]]
id   = "header"
kind = "top"
height = 40

[[egui.panels]]
id   = "main"
kind = "central"

[[egui.regions]]
id      = "param_grid"
panel   = "main"
purpose = "parameter_controls"
layout  = "grid"
columns = 2

[[egui.regions]]
id      = "scope"
panel   = "main"
purpose = "free_form"
```

Reduction kinds (after inline):

- `egui.Panel` (extends `Module`) — one per `[[egui.panels]]`.
  Qualified name is `<ModuleName>.<panel_id>`.
- `egui.Region` (extends `Module`) — one per `[[egui.regions]]`.
  Qualified name is `<ModuleName>.<region_id>`.

Reduction relation (intra-block — this is the first block in the arc
to declare its own relation):

- `egui.region_in_panel` (extends `depends_on`) — Region → Panel.
  Materialized from each region's `panel` field. The reducer's
  `REDUCE_UNRESOLVED` machinery surfaces missing-panel references
  at validate time without needing a new graph-check primitive.

Codegen prose (`codegen/rust/README.md`) describes how each shape
maps to Rust: panels become `egui::CentralPanel` / `SidePanel` /
`TopBottomPanel` constructors in the editor closure; per-region
scaffolding follows a `purpose`-driven pattern (parameter_controls
leaves a placeholder for the bridge to fill; metering, preset_selector,
free_form, label_block each have their own scaffold pattern).

## Sharp-test rationale

- **Concreteness:** ✓ — names a specific UI toolkit (`egui` via
  `nih_plug_egui`). Doesn't try to abstract over "any UI library."
- **Codegen-completeness:** ✓ — given a conforming TOML, the codegen
  prose produces working Rust source: a `create_egui_editor` call
  with the editor state struct, panels in correct ordering,
  per-region scaffolds the user fills in. No step requires
  "depends on which X."
- **No cross-block reach:** ✓ — codegen prose explicitly defers
  parameter-widget rendering to the `egui+nih-plug` bridge. The
  block's prose mentions "the bridge owns this" without naming
  specific parameters or widgets.

## Deferred (revisit triggers per §12)

- **Themed styling** — `[egui.style]` for color palettes, font
  choices, knob skins. Promote when a real plugin shows a need
  beyond hand-authored egui style code.
- **Conditional regions** — region visibility based on parameter
  values (e.g. envelope mode determines which envelope-shape regions
  show). v1 emits all regions unconditionally; conditional emission
  would require a `visible_when` predicate field.
- **Multi-section regions** — a region containing nested regions
  (collapse / expand groups). v1 keeps regions flat; nesting could
  promote later.
- **Per-region `text` field for `label_block`** — v1 leaves label
  text TODO-stubbed; promoting to a `text: [string]` field could
  remove the TODO. Bounded follow-up.
