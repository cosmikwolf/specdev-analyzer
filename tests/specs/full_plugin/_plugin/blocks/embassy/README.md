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
  framework rules differ.
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

The guidance (`guidance.md`) carries the rules for the code: tasks are
`#[embassy_executor::task]` free fns, channels are `static` declarations
of the sync type their `kind` names, and `init` spawns them.

## Sharp-test rationale

- **Concreteness:** ✓ — names a specific Rust crate (`embassy-executor`
  + `embassy-sync`), not the abstract concept of "async runtime."
- **Rule-pack completeness:** ✓ — each task and channel constrains the
  code (pool size, payload type, depth, kind); the guidance carries the
  rules no check can verify (free fns, `'static` channels, `Send` across
  awaits).
- **No cross-block reach:** ✓ — the guidance explicitly excludes
  `embassy-stm32` and other peripheral-HAL crates. Tasks and channels
  are named by their own qualified paths; peripheral wiring is the
  job of the `embassy+<peripheral-block>` bridge.

## Deferred (each with a revisit trigger)

- **Signals + Mutexes + PubSub** — covered partially via the `kind =
  "signal"` channel flavor; full first-class types come when a project
  using them surfaces real authoring friction.
- **Timers / delays** — currently captured as task body content (the
  user writes `embassy_time::Timer::after(...)` inside the task). A
  typed `[[embassy.timers]]` entry could promote this; defer until
  dogfood evidence reveals the need.
- **Multi-core / SMP** — embassy supports it, but the task/channel
  shape doesn't change; the bridge to a specific MCU block (e.g.
  `embassy+rp2040`) handles core-affinity wiring.
- **Cross-block wiring with peripheral blocks** — handled by the
  `embassy+<X>` bridges. Until a needed bridge ships, projects can
  author the glue manually inside task bodies.
