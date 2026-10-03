# eframe block

Native application-shell block targeting
[eframe](https://github.com/emilk/egui/tree/master/crates/eframe) 0.36.
Contributes a namespaced `[eframe]` table on `Module` describing the
**shell**: the root window, the repaint policy, state that survives a
restart, and any additional viewports.

## When to use this block

Inline `eframe` when your project:

- Is a native desktop application built on egui, started through
  `eframe::run_native`.
- Has a repaint policy worth stating — in particular, receives work from
  outside the UI thread and therefore needs `request_repaint` rather than
  polling.
- Persists anything across restarts: window geometry, preferences, a
  last-used value.

Don't use this block if:

- You are targeting a plugin host that supplies its own window and event
  loop. The shell is the host's, not yours, and this block's entire
  surface would be inert.
- You are building for web/wasm. v1 is native-only; the wasm entry point
  differs enough to deserve its own treatment.
- Your UI is not egui. The window vocabulary here is eframe's.

## Why shell-only (EFRAME-SHELL-ONLY)

This block models how the application **starts, wakes, persists and
windows** — not what is drawn inside. Three reasons:

1. **They are different vocabularies.** A window has a title, a size, a
   renderer and a repaint policy. What is drawn has panels, regions,
   widgets and layout. Merging them produces a schema where most fields
   are irrelevant to most entries.
2. **Layout is separable and reusable.** A description of where controls
   sit is independent of whether the host is a native window or something
   else entirely. Keeping it out of the shell block is what lets it be
   reused by a different host.
3. **The shell is small and finite.** Windows, viewports, storage keys
   and one repaint policy — a surface that can be covered completely.
   Layout is neither small nor finite.

The practical consequence: this block says nothing about panels or
widgets. It covers the `App` impl's frame — the `logic` and `ui` methods,
the options, the storage round-trip — and what goes inside `ui` comes
from elsewhere.

## What this block contributes

Schema additions on `Module`:

```toml
[eframe]
app_name       = "iroh-p2p-test"
window_title   = "iroh p2p test"
app_id         = "dev.zetaohm.p2ptest"
initial_size   = { width = 900.0, height = 600.0 }
min_size       = { width = 600.0, height = 400.0 }
resizable      = true
centered       = true
renderer       = "glow"
persist_window = true
repaint        = "reactive"

[[eframe.persisted]]
key        = "prefs"
value_type = "Prefs"

[[eframe.viewports]]
id    = "event_log"
title = "Event log"
kind  = "deferred"
initial_size = { width = 480.0, height = 320.0 }
```

Two entity kinds (`Viewport`, `Persisted`) and no relations — they are
independent of each other, following the embassy precedent.

### The repaint field is the one that matters

eframe is reactive by default: it paints on input and stops otherwise.
Anything arriving from outside the UI thread is therefore **invisible**
until something calls `request_repaint`. That single fact is behind most
"my egui app doesn't update" confusion, and the usual fix found by
copying an example — switch to continuous repaint — trades a correctness
bug for a battery bug.

Declaring the policy makes the choice deliberate. `reactive` plus a
`request_repaint` from whatever produced the work gives the same latency
as continuous repaint and idles at zero CPU.

## What this block does NOT contribute

- **Anything drawn inside a window.** Panels, widgets, layout — see the
  scoping rationale above.
- **The types stored under a persisted key.** `value_type` names a type
  the project owns.
- **Web/wasm entry points, custom fonts and visuals, window icons, menu
  bars, wgpu configuration beyond selecting the renderer.** Deferred to
  v1.x; promote when a real project needs them.
- **`egui` as a separate dependency.** eframe re-exports it and the
  versions are locked upstream, so the block contributes only `eframe`.
  This is deliberate: see "Interaction with the egui block" below.

## Sharp-test rationale

- **Leg 1 — concreteness.** `eframe` names one specific crate. "Which GUI framework?" cannot substitute — the schema
  is shaped around eframe's own concepts (`NativeOptions`,
  `ViewportBuilder`, its `Storage` round-trip, the reactive repaint
  model), none of which transfer to another toolkit.
- **Leg 2 — rule-pack completeness.** Every field constrains the code:
  window geometry, the repaint policy, each persisted key, each viewport.
  The guidance carries the rules no check can verify (logic vs painting,
  reactive repaint, load-once storage, deferred viewports).
- **Leg 3 — no cross-block reach.** The guidance names no other
  block's types. It does reference `eframe::egui::Ui` and
  `eframe::egui::Context`, but those are eframe's own re-export and
  appear in its own trait signatures — there is no way to describe
  implementing `eframe::App` without them. It reaches into no other
  block's schema, and says nothing about layout.

## Companion blocks and profiles

Pairs with the `egui` block, which describes **what is drawn** inside
the window. The two are complementary: this one ends at the window,
that one starts inside it.

### Interaction with the egui block

**This block is the egui host for a native application.** The `egui`
block models topology — panels, regions, purposes — and contributes no
crates at all, so egui arrives from here. Inline both: this block ends
at the window, that one starts inside it.

Two obligations follow, both stated in this block's guidance:

1. **The crate-root alias.** `pub use eframe::egui;` once. Code for the
   egui block refers to `crate::egui` and resolves through it. Exactly
   one host per crate has this line; a second would collide there, which
   is the intended failure.
2. **The panel API generation.** eframe 0.36 supplies egui 0.36, so the
   egui block's **modern** panel form applies — unified `egui::Panel`,
   `show` taking a `&mut Ui`, sizes via `default_size`. That matches
   what `App::ui` hands you, so the topology function takes the `ui`
   directly.

Historical note worth keeping: this pairing used to be impossible. The
`egui` block contributed `nih_plug_egui` (pulling egui 0.31) while this
block pulls egui 0.36, so co-inlining put two semver-incompatible eguis
in one graph — and because the two blocks declared *different crate
names*, the manifest merge saw no conflict and nothing warned. The
collision was transitive and only visible in the resolved graph. That is
why the egui block now contributes no version at all and the host owns
it.

### Profiles

Wants a profile for a runnable application. Use **`rust-app`**: a
`[lib]` testable core plus a `[[bin]]` entry point in one crate, which is the shape an eframe application has — headless logic in
the lib, the `run_native` call in the bin. A member of a workspace can
carry this block too.

## Deferred

- Web/wasm target and `WebRunner`.
- Fonts, visuals and theme as declared surface rather than hand-authored
  setup in the creator closure.
- Window icon, which needs an asset reference the schema has no vocabulary
  for yet.
- Per-viewport persistence, which would be the first real candidate for
  an intra-block relation (`Persisted` → `Viewport`).
