# egui+nih-plug bridge

Pairwise wiring between the `egui` block (editor topology) and the
`nih-plug` block (plugin parameters + audio I/O). Contributes
parameter ↔ widget bindings under
`bindings."egui+nih-plug".parameter_widgets[]` on plugin shell
modules.

This is the **architectural validation moment** for the bridge
primitive. Unlike `embassy+stm32h5-rs` (which is closer to a wiring
stub — peripheral owned-by task), this bridge carries a load-bearing
semantic role: parameter↔widget binding is the cross-cutting concern
that neither source block can express alone. nih-plug owns parameters
but not rendering; egui owns layout but not parameters; only the
bridge knows both.

## When to use this bridge

Inline `egui+nih-plug` when:

- Your plugin shell module has both a `[nih_plug]` table (parameters)
  and an `[egui]` table (editor topology).
- You want declarative parameter↔widget binding (knob, slider, toggle,
  button, dropdown, radio_group, numeric_field, value_display per D50)
  rather than hand-authoring the egui calls inside `free_form` regions.

Don't inline if:

- Your editor is entirely custom-drawn (no parameter widgets) — the
  egui block's `free_form` regions handle this.
- You're not using egui (using iced / vizia / raw skia). A different
  bridge would handle that pairing.

## What this bridge contributes

Schema additions on `Module`:

```toml
[[bindings."egui+nih-plug".parameter_widgets]]
parameter   = "gain"               # references nih_plug.parameters[].id
region      = "param_grid"         # references egui.regions[].id
widget_kind = "knob"               # 8-value closed enum (D50)
range_hint  = "logarithmic"        # optional; defaults from parameter.smoother
label       = "above"              # optional; default per region.layout
```

Reduction relation:

- `egui+nih-plug.widget_binds_parameter` (extends `depends_on`) —
  Region → Parameter. Materialized per binding; carries `widget_kind`
  / `range_hint` / `label` as relation attributes. The reducer's
  `REDUCE_UNRESOLVED` machinery surfaces missing-region or missing-
  parameter references at validate time without a new graph-check
  primitive.

Codegen prose (`codegen/rust/README.md`) describes how each binding
maps to Rust egui calls. The 8 widget kinds (D50) each get one
emission stanza:

| widget_kind     | egui call                                                                |
|-----------------|--------------------------------------------------------------------------|
| `knob`          | `ui.add(egui_extras::Knob::new(...))` — needs `egui_extras` project dep   |
| `slider`        | `ui.add(widgets::ParamSlider::for_param(...))`                            |
| `toggle`        | `ui.add(widgets::ParamButton::for_param(...))`                            |
| `button`        | `ui.button(...)` with momentary `setter.set_parameter` semantics          |
| `dropdown`      | `ui.add(widgets::ParamComboBox::for_param(...))`                          |
| `radio_group`   | `for variant in EnumType::variants() { ui.radio_value(...) }` row         |
| `numeric_field` | `egui::DragValue::new(...).clamp_range(...)` wired through setter         |
| `value_display` | `ui.label(format!("{}{}", value, unit))` — read-only                      |

## Sharp-test rationale (doc 15 §3.1)

- **Concreteness:** ✓ — names two specific blocks (egui + nih-plug)
  and a specific widget vocabulary (D50's 8 kinds).
- **Codegen-completeness:** ✓ — given a conforming TOML, the codegen
  prose produces working Rust source: per-binding widget calls
  emitted into the egui block's `parameter_controls` region
  placeholders. No step requires "depends on which X."
- **Load-bearing semantic role:** ✓ (vs `embassy+stm32h5-rs`'s
  wiring-stub role). The bridge is the only place that knows both
  parameter type AND widget rendering — neither source block has
  enough context to emit the binding alone.

## Validate-time enforcement

Two layers of validation in v1:

1. **Reducer (REDUCE_UNRESOLVED)** — bindings that reference a
   non-existent parameter or a non-existent region surface as
   `unresolved Parameter endpoint <id>` / `unresolved Region endpoint
   <id>` at validate time. This covers the load-bearing
   referential checks (NIH-EGUI-001 / NIH-EGUI-002 equivalents in the
   spike's framing).

2. **Codegen-prose discipline rules** — the bridge's emission code
   rejects:
   - Bindings into a region whose `purpose` ≠ `parameter_controls`.
   - `widget_kind` ↔ `parameter.kind` mismatches (e.g.
     `widget_kind = "toggle"` on a `kind = "float"` parameter).

These are NOT graph-check primitives in v1 — promotion to validate-
time enforcement (NIH-EGUI-003..009 codes) is a bounded follow-up per
D50. Trigger: first dogfood that produces a misbinding the codegen-
prose discipline doesn't catch.

## Co-residency requirement (v1)

Both `[nih_plug]` and `[egui]` tables MUST be on the SAME module (the
plugin_shell-layer module). The bindings table lives on that same
module. v1 does NOT support cross-module bindings (e.g. an engine
module's parameter rendered in a shell module's editor). This is a
named bounded follow-up (spike §7) — trigger to commit cross-module
bindings is the first dogfood needing engine-published metering
surfaced in the editor.

## Deferred (revisit triggers per spike §7 + D50)

- **Validate-time enforcement of NIH-EGUI-003..009.** v1 catches them
  in codegen prose; promotion to graph-check primitives requires
  designing the cross-attribute primitive (relation attribute vs.
  target entity attribute joint check).
- **Multi-parameter widgets.** XY pads (2 parameters), envelope
  editors (N parameters). Different binding family — needs
  `parameter_x` / `parameter_y` or `parameters: [...]` array form.
  Trigger: first dogfood needing in-spec multi-parameter widgets.
- **Audio-thread-state widgets.** LEDs, meters, graph displays bind
  to `Atomic*` cells published by the audio thread, not parameters.
  Different binding family entirely. Today's escape hatch:
  `purpose = "metering"` regions in the egui block + hand-authored
  rendering.
- **Cross-module bindings.** v1 enforces co-residency. Promote when
  a real plugin needs it.
- **Style attributes per binding.** Knob diameter, slider thickness,
  custom colors. Out of scope for v1 — emerge from a real plugin's
  cosmetic needs.
- **Wider `widget_kind` enum.** D50 ships 8 kinds; bounded follow-up
  for additional 1:1 kinds (radio buttons styled as buttons,
  segmented controls, etc.).

## References

- Phase 15-A spike: `specdev-next/000-audio-plugin-spike-2026-05-03.md`
- D47: egui topology-only (parameter→widget bindings live in this bridge)
- D50: widget_kind enum widening (8 kinds, NIH-EGUI-001..009)
- nih-plug widgets: `nih_plug::widgets` module
- egui: https://github.com/emilk/egui
