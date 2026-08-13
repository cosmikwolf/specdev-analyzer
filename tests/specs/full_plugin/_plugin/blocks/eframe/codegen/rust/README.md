# eframe — Rust codegen conventions

These conventions describe how an `eframe` block table on a module spec
maps to Rust source. They layer on top of the project's profile's
`module.conventions.md`; concerns specific to the native application shell
(window construction, the repaint policy, persisted state, additional
viewports) live here.

Pinned to **eframe 0.36**, which requires rustc 1.95 or newer.

## Crate dependencies

This block contributes `eframe` and `serde`:

```toml
eframe = { version = "0.36", features = ["persistence", "glow"] }
serde = { version = "1", features = ["derive"] }
```

**Do NOT** add `egui` as a separate dependency. eframe re-exports it as
`eframe::egui` and the two versions are locked together upstream; a
separate declaration lets a project end up with two incompatible egui
versions whose types do not unify, which fails far from its cause. Write
`eframe::egui::...` and take the re-export.

The `persistence` feature is what provides the storage helpers — without
it they do not exist. Emit it whenever the spec declares any persisted
entry.

## Host obligation — the crate-root egui alias

This block is a **host** for the `egui` block, which models topology
and contributes no crates of its own. Emit exactly one line at the
crate root:

```rust
pub use eframe::egui;
```

Emit it whether or not the `egui` block is inlined — it is cheap, and
it is what that block's emitted code resolves through (`crate::egui`).
Exactly one host per crate emits this; a second host would collide at
this line, which is the intended failure rather than two
semver-incompatible eguis resolving silently.

**Which panel form the egui block uses:** eframe 0.36 supplies egui
0.36, so its **modern** form applies — unified `egui::Panel`, `show`
taking a `&mut Ui`, sizes via `default_size`. That is the same `&mut Ui`
`App::ui` hands you, so the topology function takes it directly:

```rust
fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut Frame) {
    crate::<module>::draw(ui);
}
```

The egui block's prose is the source for the panel tables; this section
only fixes which of its two forms is in play.

## The App trait

An application implements `eframe::App`. In 0.36 the trait has two frame
methods and **no `update` method** — that was the pre-0.35 shape:

- `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame)` — **required**.
  Painting. Called each time the UI needs repainting.
- `fn logic(&mut self, ctx: &egui::Context, frame: &mut Frame)` —
  optional. Runs once before each `ui`, **and** when the window is hidden
  but a repaint was requested.

```rust
use eframe::egui;
use eframe::{App, Frame};

impl App for DemoApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut Frame) {
        let _ = ctx;
        self.frames += 1;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut Frame) {
        ui.label(format!("frames: {}", self.frames));
    }
}
```

Put state that arrives from elsewhere into `logic`, not `ui`. `logic`
still runs while the window is hidden, so nothing queues up unbounded
while the app is minimised — whereas work done only in `ui` stops
entirely.

**Do NOT** emit an `update` method. It does not exist in this pin and
will not be called; the code compiles as an inherent method and the
window renders blank, which is a slow thing to diagnose.

**Do NOT** do blocking work in either method. Both run on the UI thread;
an await-free blocking read freezes the window.

## Window emission

The `[eframe]` scalars become `NativeOptions` plus the root
`ViewportBuilder`. Geometry lives on the builder; the rest are
`NativeOptions` fields.

```rust
use eframe::egui::{vec2, ViewportBuilder};
use eframe::{NativeOptions, Renderer};

NativeOptions {
    viewport: ViewportBuilder::default()
        .with_title("iroh p2p test")
        .with_app_id("dev.zetaohm.p2ptest")
        .with_inner_size(vec2(900.0, 600.0))
        .with_min_inner_size(vec2(600.0, 400.0))
        .with_resizable(true),
    centered: true,
    persist_window: true,
    renderer: Renderer::Glow,
    ..Default::default()
}
```

`app_name` is NOT a window field — it is the first argument to the run
call, and it also determines where persisted state lives on disk. Emit
`window_title` on the builder and `app_name` at the entry point; they are
different strings for different purposes.

Sizes are logical points as `f32`, so emit `vec2(900.0, 600.0)`, never
integer literals. Omit `.with_min_inner_size(...)` entirely when the spec
declares no `min_size` rather than emitting a zero.

**Do NOT** emit `..Default::default()` before the explicitly-set fields —
struct update syntax must come last, and the compiler error it produces
does not name the real problem.

## Repaint emission

eframe is reactive by default: it paints on input and stops otherwise.
The `repaint` field decides what else is emitted, at the end of `logic`:

| repaint | emission |
|---|---|
| reactive | nothing |
| continuous | `ctx.request_repaint()` |
| interval | `ctx.request_repaint_after(Duration::from_millis(n))` |

```rust
use std::time::Duration;

// repaint = "continuous"
ctx.request_repaint();

// repaint = "interval", repaint_interval_ms = 250
ctx.request_repaint_after(Duration::from_millis(250));
```

For `reactive`, whatever produces work off the UI thread must wake it.
`egui::Context` is cheap to clone and is `Send + Sync`, so hand a clone to
that worker and have it call `request_repaint` when it has something:

```rust
pub fn wake_on_external_work(ctx: egui::Context) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        // ... whatever produced the work.
        ctx.request_repaint();
    })
}
```

That is the whole mechanism: a reactive app displays data arriving from
anywhere, with no polling, provided the producer calls `request_repaint`.
Where the work comes from is not this block's concern — emit the wake
path and let the module that owns the producer drive it.

**Do NOT** emit `continuous` merely because state changes off-thread.
A reactive app plus `request_repaint` gives the same latency and idles at
zero CPU; continuous repaint spins a core forever and, on a laptop, is
the difference between hours of battery and one.

**Do NOT** call `request_repaint` from inside `ui` on every pass. That is
`continuous` written indirectly, and it defeats the reactive default
without saying so.

## Persistence emission

Each `[[eframe.persisted]]` entry becomes one typed round-trip: read once
at construction from `CreationContext::storage`, written back in
`App::save`.

```rust
use eframe::{App, CreationContext, Frame, Storage};
use serde::{Deserialize, Serialize};

pub const PREFS_KEY: &str = "prefs";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Prefs {
    pub last_ticket: String,
    pub window_scale: f32,
}

impl PersistApp {
    pub fn new(cc: &CreationContext<'_>) -> Self {
        let prefs: Prefs = cc
            .storage
            .and_then(|storage| eframe::get_value::<Prefs>(storage, PREFS_KEY))
            .unwrap_or_default();
        Self { prefs }
    }
}

impl App for PersistApp {
    fn save(&mut self, storage: &mut dyn Storage) {
        eframe::set_value(storage, PREFS_KEY, &self.prefs);
    }
}
```

`default_on_error = true` (the default) is the `.unwrap_or_default()`
above. The `value_type` is used verbatim and must derive `Serialize` and
`Deserialize`; the type belongs to the project, not to this block.

**Do NOT** read storage anywhere but the constructor. `CreationContext`
is the only place it is offered, and re-reading mid-run would overwrite
live state with a stale snapshot.

**Do NOT** emit a `?` or an `unwrap` on the load path when
`default_on_error` is true. A corrupt or absent value must leave the
application starting normally, not refusing to open.

## Viewport emission

Each `[[eframe.viewports]]` entry becomes one additional OS window, shown
from the parent's painting code. Note the 0.36 callback takes `&mut Ui`,
not a context — a child paints the same way the root does.

```rust
use eframe::egui::{self, ViewportBuilder, ViewportClass, ViewportId};

ctx.show_viewport_deferred(
    ViewportId::from_hash_of("event_log"),
    ViewportBuilder::default()
        .with_title("Event log")
        .with_inner_size(egui::vec2(480.0, 320.0)),
    |ui: &mut egui::Ui, _class: ViewportClass| {
        ui.label("child viewport contents");
    },
);
```

`ViewportId::from_hash_of` takes the spec's `id`, so the identifier stays
stable across runs. The callback is `Send + Sync + 'static`, so it cannot
borrow the app — move a clone of whatever it needs, or pass shared state
behind a handle.

**Do NOT** emit `show_viewport_immediate` for a `kind = "deferred"`
entry. An immediate viewport re-runs inside the parent's pass, so a slow
child stalls the root window — the opposite of what a separate window is
usually for.

## Entry point and ownership

The run call takes `app_name`, the options, and a creator closure that
builds the app. The creator receives `CreationContext`, which is where
storage and the context are available:

```rust
pub fn run(options: NativeOptions) -> eframe::Result {
    eframe::run_native(
        "demo-app",
        options,
        Box::new(|cc: &CreationContext<'_>| {
            let _ = cc;
            Ok(Box::new(DemoApp { frames: 0 }))
        }),
    )
}
```

The run call owns the main thread and does not return until the last
window closes, so it belongs at the end of the entry point. Anything that
must outlive the UI is started before it and handed in through the
creator closure.

**Do NOT** emit work after the run call expecting it to execute during
the session — it runs at shutdown, if at all. Cleanup that must happen
while windows are open belongs in `App::save` or `App::on_exit`.
