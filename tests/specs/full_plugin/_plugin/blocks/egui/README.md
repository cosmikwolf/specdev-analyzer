# egui block

Editor-topology block targeting [egui](https://github.com/emilk/egui).
Contributes a namespaced `[egui]` table on `Module` describing
**topology** — panels and the regions inside them — for any module that
draws an egui interface.

The block is **host-neutral**: it describes what is drawn and where, not
what opens the window or supplies the frame. That is a host's job, and
egui has more than one host.

## When to use this block

Inline `egui` when your project:

- Draws an egui interface with a deterministic structure — known panels,
  known regions inside them, known purposes. egui suits both
  immediate-mode UIs and dashboard-style layouts.
- Pairs it with a host (see below). This block never opens a window and
  never supplies an egui frame.

Don't use this block if:

- Your interface is hand-rolled with no panel/region structure (a single
  `CentralPanel` with hand-coded layout). The block adds no value
  there; an `[egui] enabled = true` flag would be lighter, but the
  sharp-test rule says blocks must contribute concrete shapes.
- You're targeting a different UI toolkit (iced, vizia, raw skia). Those
  would land as separate blocks; the topology vocabulary would likely
  differ enough to justify it.

## This block needs a host (EGUI-NEEDS-A-HOST)

egui does not run on its own — something must own the window or the
frame and hand it a `Context` each paint. This block therefore
contributes **no crates at all**, and is the one shipped block that is
deliberately not usable by itself. Two shipped units are hosts:

| Host | What it is | Supplies egui as | Unit to inline |
|------|------------|------------------|----------------|
| eframe | native desktop application shell | `eframe::egui` | the `eframe` block |
| nih-plug | audio-plugin editor | `nih_plug_egui::egui` | the `egui+nih-plug` bridge |

Every host emits one line at the crate root:

```rust
pub use eframe::egui;          // eframe host
pub use nih_plug_egui::egui;   // nih-plug host
```

and this block's emitted code refers to **`crate::egui`**. Two
consequences worth understanding, because both are the point:

1. **Emission does not vary by host.** The panel and region code this
   block produces is identical whichever host is in play, so the block
   never has to ask which one it is.
2. **Two hosts in one crate fail loudly.** They collide at that single
   `pub use` line with a duplicate-definition error, rather than
   resolving two semver-incompatible egui versions whose `Context` and
   `Ui` types silently refuse to unify somewhere far downstream.

### When no host block is inlined — the project is the host

egui runs under plenty of integrations this tree ships no block for: a
hand-rolled `winit` + `egui-wgpu` / `egui_glow` stack, an existing
application that already owns its window and event loop and wants egui
as a tools overlay, `bevy_egui`, `egui-baseview` without nih-plug, or
headless use in tests. The block works there — topology is topology —
but nothing supplies the crate.

Then the project declares `egui` itself under `architecture.toml`
`[[external_deps]]`, which makes it a **direct** dependency, and
emitted code uses plain **`egui::`** with no alias. It must *not*
hand-add a crate-root `pub use`: with no unit owning that line it
would sit in a spec-generated, lineage-recorded crate root and the next
`/spec-gen` would delete it.

Worth knowing when picking an integration: **not every one re-exports
egui.** `eframe`, `nih_plug_egui`, `egui-winit` and `bevy_egui` do;
`egui-wgpu` and `egui_glow` do not. A host block can only be written
for an integration that re-exports — otherwise there is no path for the
alias to point at, and the direct declaration above is the only option.

### The host also decides which panel API is in play (EGUI-API-GENERATION)

egui changed its panel API, and the two shipped hosts sit one on each
side of it: `nih_plug_egui` supplies egui 0.31 (`SidePanel` /
`TopBottomPanel`, `show` taking a `&Context`), while `eframe` 0.36
supplies egui 0.36 (unified `Panel`, `show` taking a `&mut Ui`).

The codegen prose carries **both forms** and says how to tell which
applies. Both are compiled in this block's reference crate, so neither
is written from memory. Nothing a spec author writes changes — the
`[egui]` table, the entities and the relation are identical either
way; only the emitted calls differ.

This is why the block is not split in two: the divergence is in the
emission, not in the spec surface.

## Why topology only (EGUI-TOPOLOGY-ONLY)

The egui block models topology — WHERE things live — but NOT WHICH
widgets render WHICH values. That cross-cutting concern belongs to
whatever owns the values: for an audio plugin, the `egui+nih-plug`
bridge. Three reasons:

1. **egui is a UI library, not a widget catalog.** Panel + region
   layout is the right abstraction for a block. Widget content is
   too fine-grained and varies too widely (knob style, label
   placement, custom drawing) to schema usefully without ballooning
   the surface.
2. **Value→widget binding is exactly what bridges are for.** It wires
   the owner of the values to the owner of the rendering. A bridge
   primitive is the right home for cross-cutting wiring.
3. **`free_form` regions accommodate custom drawing** without forcing
   it into the spec. Oscilloscopes / plots / spectrum analyzers / XY
   pads / graph views are declared structurally — "this region exists,
   content is hand-authored" — but Claude can still emit a coherent
   scaffold around them.

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

`default_size` and `resizable` describe the **drawn surface's**
preferred size. A host that declares its own window geometry — an
`[eframe]` window does — wins, and these are ignored.

Reduction kinds (after inline):

- `egui.Panel` (extends `Module`) — one per `[[egui.panels]]`.
  Qualified name is `<ModuleName>.<panel_id>`.
- `egui.Region` (extends `Module`) — one per `[[egui.regions]]`.
  Qualified name is `<ModuleName>.<region_id>`.

Reduction relation (intra-block):

- `egui.region_in_panel` (extends `depends_on`) — Region → Panel.
  Materialized from each region's `panel` field. The reducer's
  `REDUCE_UNRESOLVED` machinery surfaces missing-panel references
  at validate time without needing a new graph-check primitive.

Codegen prose (`codegen/rust/README.md`) describes how each shape
maps to Rust: panels become `egui::CentralPanel` / `SidePanel` /
`TopBottomPanel` constructors inside whatever paint callback the host
supplies; per-region scaffolding follows a `purpose`-driven pattern.

## What this block does NOT contribute

- **A window, an event loop, or an egui frame.** See
  `EGUI-NEEDS-A-HOST` above.
- **Any crate**, egui included. The host owns the version.
- **Widget-to-value bindings.** See `EGUI-TOPOLOGY-ONLY` above.

## Sharp-test rationale

- **Concreteness:** ✓ — names a specific UI toolkit (`egui`). Doesn't
  try to abstract over "any UI library"; the vocabulary is egui's own
  (panels, regions, immediate-mode layout closures).
- **Codegen-completeness:** ✓ with one stated condition — given a
  conforming TOML, the codegen prose produces working Rust source:
  panels in correct ordering, per-region scaffolds the user fills in.
  The condition is `EGUI-API-GENERATION`: panel emission has two forms
  and the prose branches on which egui the host supplies. That is
  **determinate, not underspecified** — a project has exactly one host,
  the host fixes the egui version, and the prose states a feature check
  that decides it. Both branches are compiled. Everything else, the
  `crate::egui` alias included, is host-independent.
- **No cross-block reach:** ✓ — the codegen prose names no host's
  types and no other block's schema. It defers value-bound widget
  rendering to a bridge without naming specific values or widgets.

## Deferred (each carries a revisit trigger)

- **More region purposes** — `list`, `log`, `plot`, `form` alongside
  the five shipped. Each needs its own scaffold, and `free_form`
  covers all of them adequately today. Promote when a project reports
  the five are inadequate for its interface.
- **Themed styling** — `[egui.style]` for color palettes, font
  choices, widget skins. Promote when a real project shows a need
  beyond hand-authored egui style code.
- **Conditional regions** — region visibility based on state (e.g. a
  mode selection determines which regions show). v1 emits all regions
  unconditionally; conditional emission would require a `visible_when`
  predicate field.
- **Multi-section regions** — a region containing nested regions
  (collapse / expand groups). v1 keeps regions flat; nesting could
  promote later.
- **Panels inside a host's child window.** v1 draws every panel into
  the host's root surface. A host that declares child windows (an
  `[eframe]` viewport does) would need a Panel → child-window relation,
  which is cross-unit wiring and therefore a bridge, not a field here.
- **Per-region `text` field for `label_block`** — v1 leaves label
  text TODO-stubbed; promoting to a `text: [string]` field could
  remove the TODO. Bounded follow-up.
