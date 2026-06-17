# embassy block

Async runtime block targeting [embassy](https://embassy.dev), the
async/await framework for embedded Rust. Contributes a namespaced
`[embassy]` table on `Module` with two concerns: **tasks** (long-lived
async fns spawned by the runtime) and **channels** (inter-task message
passing).

## When to use this block

Inline `embassy` when your project:

- Uses `embassy-executor` for async/await firmware.
- Has long-running concurrent activities expressed as separate tasks
  rather than a single main loop.
- Passes data between tasks via typed channels rather than shared
  mutable state.

Don't use embassy if:

- You're targeting a different async runtime (RTIC, Tokio in std). A
  separate block (`rtic`, `tokio-runtime`) is the right home; the
  conceptual shapes overlap (`embassy.Task` and `rtic.Task`) but the
  codegen mappings differ.
- Your firmware is single-loop synchronous. The block adds runtime
  overhead and async syntax that buy nothing in that case.

## What this block contributes

Schema additions on `Module`:

```toml
[[embassy.tasks]]
name = "read_sensor"
pool_size = 1            # optional; defaults to 1
priority = "normal"      # optional; low | normal | high

[[embassy.channels]]
name = "events"
payload_type = "Event"   # any Rust type expression
depth = 16
kind = "bounded"         # bounded | watch | signal; defaults to bounded
```

Reduction kinds (after inline):

- `embassy.Task` (extends `Module`) — one per `[[embassy.tasks]]` entry.
  Qualified name is `<ModuleName>.<task_name>`.
- `embassy.Channel` (extends `Module`) — one per `[[embassy.channels]]`.
  Qualified name same shape.

Codegen prose (`codegen/rust/README.md`) describes how each shape maps
to Rust: tasks become `#[embassy_executor::task]` free fns, channels
become `static CHANNEL: Channel<...>` declarations, init wires them
together at spawn time.

## Sharp-test rationale (doc 15 §3.1)

- **Concreteness:** ✓ — names a specific Rust crate (`embassy-executor`
  + `embassy-sync`), not the abstract concept of "async runtime."
- **Codegen-completeness:** ✓ — given a conforming TOML, the codegen
  prose produces working Rust source: task fns with the right
  attribute, statics with the right type, an init fn that spawns. No
  step requires "depends on which X."
- **No cross-block reach:** ✓ — codegen prose explicitly excludes
  `embassy-stm32` and other peripheral-HAL crates. Tasks and channels
  are named by their own qualified paths; peripheral wiring is the
  job of the `embassy+<peripheral-block>` bridge (Phase 7+).

## Deferred (revisit triggers per §12)

- **Signals + Mutexes + PubSub** — covered partially via the `kind =
  "signal"` channel flavor; full first-class types come when a project
  using them surfaces real authoring friction.
- **Timers / delays** — currently captured as task body content (the
  user writes `embassy_time::Timer::after(...)` inside the task). A
  typed `[[embassy.timers]]` entry could promote this; defer until
  Phase 8.5 dogfood + Phase 12 firmware integration test reveal the
  need.
- **Multi-core / SMP** — embassy supports it, but the task/channel
  shape doesn't change; the bridge to a specific MCU block (e.g.
  `embassy+rp2040`) handles core-affinity wiring.
- **Cross-block wiring with peripheral blocks** — the `embassy+<X>`
  bridges are Phase 7+ work. Until they ship, projects can author the
  glue manually inside task bodies.
