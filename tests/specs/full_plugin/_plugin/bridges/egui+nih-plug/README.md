# egui+nih-plug bridge

Pairwise wiring between the `egui` block (interface topology) and the
`nih-plug` block (plugin parameters + audio I/O). Contributes
parameter ↔ widget bindings under
`bindings."egui+nih-plug".parameter_widgets[]` on plugin shell
modules.

It is also **the egui host for a plugin**. The `egui` block is
host-neutral and contributes no crates, so this bridge supplies
`nih_plug_egui` — the join product of the pairing, an adapter that
exists only because a nih-plug plugin hosts an egui interface — along
with the crate-root `pub use nih_plug_egui::egui;` alias and the
`create_egui_editor` closure the topology draws into. Neither endpoint
block may own that crate: `nih-plug` must stay UI-agnostic and `egui`
must stay host-agnostic.

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
  button, dropdown, radio_group, numeric_field, value_display per WIDGET-KIND-V1)
  rather than hand-authoring the egui calls inside `free_form` regions.

- Your plugin ships an egui editor at all, **even with no parameter
  widgets yet**. The bindings table may be empty; the host binding is
  what makes the stack compile, and it lives here.

Don't inline if:

- You're not using egui (using iced / vizia / raw skia). A different
  bridge would handle that pairing.
- Your plugin ships no editor.

## What this bridge contributes

Schema additions on `Module`:

```toml
[[bindings."egui+nih-plug".parameter_widgets]]
parameter   = "gain"               # references nih_plug.parameters[].id
region      = "param_grid"         # references egui.regions[].id
widget_kind = "knob"               # 8-value closed enum (WIDGET-KIND-V1)
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

The guidance (`guidance.md`) says how each binding becomes egui calls in the code. The 8 widget kinds
(WIDGET-KIND-V1) and the parameter kinds each accepts:

| widget_kind     | parameter kind | in the code |
|-----------------|----------------|-------------|
| `knob`          | float, int     | the adapter's parameter slider (no rotary primitive ships) |
| `slider`        | float, int     | the adapter's parameter slider |
| `toggle`        | bool           | a plain checkbox wired through the setter |
| `button`        | bool           | a button with momentary set semantics |
| `dropdown`      | enum           | a combo box over the enum's variants, wired through the setter |
| `radio_group`   | enum           | a radio row over the enum's variants |
| `numeric_field` | float, int     | a drag value clamped to the range, wired through the setter |
| `value_display` | any            | a read-only label |

## Sharp-test rationale

- **Concreteness:** ✓ — names two specific blocks (egui + nih-plug)
  and a specific widget vocabulary (WIDGET-KIND-V1's 8 kinds).
- **Rule-pack completeness:** ✓ — each binding constrains the code
  (which widget, in which region, for which parameter), and the kind and
  purpose rules are stated as checkable rules.
- **Load-bearing semantic role:** ✓ (vs `embassy+stm32h5-rs`'s
  wiring-stub role). The bridge is the only place that knows both
  parameter type AND widget rendering — neither source block has
  enough context to state the binding alone.

## Validate-time enforcement

Two layers of validation in v1:

1. **Reducer (REDUCE_UNRESOLVED)** — bindings that reference a
   non-existent parameter or a non-existent region surface as
   `unresolved Parameter endpoint <id>` / `unresolved Region endpoint
   <id>` at validate time. This covers the load-bearing
   referential checks (NIH-EGUI-001 / NIH-EGUI-002 equivalents in the
   spike's framing).

2. **Guidance rules** — the bridge's guidance refuses:
   - Bindings into a region whose `purpose` ≠ `parameter_controls`.
   - `widget_kind` ↔ `parameter.kind` mismatches (e.g.
     `widget_kind = "toggle"` on a `kind = "float"` parameter).

These are NOT checks yet — an attribute-mapping check between related entities is
what turns them into checks (NIH-EGUI-003..009).

## Co-residency requirement (v1)

Both `[nih_plug]` and `[egui]` tables MUST be on the SAME module (the
plugin_shell-layer module). The bindings table lives on that same
module. v1 does NOT support cross-module bindings (e.g. an engine
module's parameter rendered in a shell module's editor).

A misplaced binding surfaces at validate time as
`REDUCE_UNRESOLVED: unresolved Parameter endpoint <id>` (or
`unresolved Region endpoint <id>`) with a follow-up `hint:` line that
names this bridge — fix by moving the bindings table to the module
that owns both `[nih_plug]` and `[egui]`, or by moving the missing
declaration onto the module that owns the bindings table.

Cross-module support is a named bounded follow-up — trigger to commit
cross-module bindings is the first dogfood needing engine-published
metering surfaced in the editor.

## Deferred (revisit triggers, including WIDGET-KIND-V1)

- **Validate-time enforcement of NIH-EGUI-003..009.** Stated in the
  guidance today; an attribute-mapping check between related
  entities is the planned check.
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
- **Wider `widget_kind` enum.** WIDGET-KIND-V1 ships 8 kinds; bounded follow-up
  for additional 1:1 kinds (radio buttons styled as buttons,
  segmented controls, etc.).

## References

- nih-plug widgets: `nih_plug::widgets` module
- egui: https://github.com/emilk/egui
