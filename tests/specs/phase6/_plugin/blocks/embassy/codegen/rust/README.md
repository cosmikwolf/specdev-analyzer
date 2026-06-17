# embassy — Rust codegen conventions

These conventions describe how an `embassy` block table on a module spec
maps to Rust source. They layer on top of the firmware-layered profile's
`module.conventions.md`; concerns specific to async runtimes (task
attributes, static channel declarations, spawning) live here.

embassy's codegen departs from the firmware-layered convention that
"actions become methods on the module struct" — embassy tasks are free
async functions, not methods. This is intentional and discussed in §6.

## Crate dependencies

This block contributes three crates: `embassy-executor`, `embassy-sync`,
`embassy-time`.

```toml
embassy-executor = { version = "0.10", features = ["platform-cortex-m", "executor-thread"] }
embassy-sync = "0.8"
embassy-time = "0.5"
```

The exact features depend on the target. Single-core Cortex-M projects
use `platform-cortex-m` + `executor-thread`; multi-core or async-from-interrupt
contexts add features per the embassy book. **Do NOT** add `embassy-stm32`
or other peripheral-HAL crates from this block — that pairing is owned by
the relevant bridge (e.g. `embassy+stm32h5-rs`).

### Project-level integration

embassy is a runtime layer; it sits on top of `[platform]` (Cortex-M
core/runtime crates) and `[runtime]` (panic handler, optional
critical-section impl) declared in `architecture.toml`. This block
does **not** declare those — read them from architecture and integrate
as follows:

- **Panic handler.** `[runtime].panic_handler` is the user's pick
  (panic-probe, panic-halt, panic-rtt-target, etc.). The module's init
  side does nothing; the panic handler crate registers itself via
  `use <crate> as _;` at crate root, emitted by the firmware-layered
  profile's main.rs scaffolding.
- **Critical section.** embassy uses `critical-section` for ISR-safe
  primitives, which needs a registered implementation or the link fails
  with `undefined symbol: _critical_section_1_0_acquire`. On
  single-core Cortex-M the implementation is `cortex-m`'s
  `critical-section-single-core` feature — but it is **not** on by
  default. The project must enable it explicitly in `architecture.toml`:
  add `features = ["critical-section-single-core"]` to
  `[platform.mcu_core]` (the cortex-m dep). If
  `[runtime].critical_section_impl` is set instead, the project picked
  an override crate — do not also enable the cortex-m feature (two
  implementations also fail to link).
- **Allocator (optional).** If `[runtime].allocator` is set, embassy
  tasks may use `alloc::*` types. Without it, channels and statics are
  the only persistent surface.

Do **not** name specific panic-handler or critical-section crates in
this block's emissions — those are project-level decisions, not
embassy decisions.

## Task emission

Each `[[embassy.tasks]]` entry becomes a free function with the
`#[embassy_executor::task]` attribute, NOT a method on the module struct.
The function body is left to the project author; codegen scaffolds the
signature and the spawning glue.

Worked example — task `read_sensor` in module `SensorReader` with
`pool_size = 1`:

```rust
#[embassy_executor::task]
async fn read_sensor(/* params from author */) {
    loop {
        // user code
    }
}
```

For `pool_size > 1`, the macro takes the size:

```rust
#[embassy_executor::task(pool_size = 4)]
async fn worker(id: u8) {
    // ...
}
```

The qualified entity name (e.g. `SensorReader.read_sensor`) drives module
boundaries: the function is emitted in `src/<layer>/<module>.rs`, but as
a `pub(crate) async fn` (or `pub async fn` for cross-crate workspaces).
**Do NOT** emit tasks inside an `impl` block on the module struct —
embassy's task macro requires a free function; placing it in an impl
breaks the spawn pattern.

## Channel emission

Each `[[embassy.channels]]` entry becomes a `static` declaration of the
appropriate type from `embassy-sync`, depending on the `kind` attribute:

| kind     | type                                                                       |
|----------|----------------------------------------------------------------------------|
| bounded  | `embassy_sync::channel::Channel<CriticalSectionRawMutex, T, N>`            |
| watch    | `embassy_sync::watch::Watch<CriticalSectionRawMutex, T, N_RECEIVERS>`      |
| signal   | `embassy_sync::signal::Signal<CriticalSectionRawMutex, T>`                 |

`T` is the `payload_type` attribute (used verbatim as a Rust type
expression); `N` is the `depth` attribute.

Worked example — `[[embassy.channels]] name = "events", payload_type = "Event", depth = 16, kind = "bounded"`:

```rust
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

static EVENTS: Channel<CriticalSectionRawMutex, Event, 16> = Channel::new();
```

The static name uppercases the channel name (`events` → `EVENTS`). Place
the static at module scope, above any function definitions.

**Do NOT** declare channels inside `impl` blocks or as struct fields —
embassy channels need `'static` lifetime to outlive task references, and
struct fields with non-`'static` references force lifetime annotations
that defeat the runtime's design.

## Spawning at init

The module's init function is the canonical spawn site. Pass a
`&Spawner` reference (from `embassy_executor::Spawner`); call
`spawner.spawn(<task>(<args>).unwrap())` per task.

Note the `.unwrap()` placement: with `embassy-executor` 0.10+, a task
function returns `Result<SpawnToken, SpawnError>` and `Spawner::spawn`
consumes the `SpawnToken` and returns `()`. So the `unwrap()` goes on
the **task call** (inside `spawn(...)`), not on the `spawn` call. The
older `spawner.spawn(task()).unwrap()` shape does not compile against
0.10.

Worked example — module `SensorReader` with one `read_sensor` task that
publishes to channel `events`:

```rust
pub fn init(spawner: &embassy_executor::Spawner) {
    spawner.spawn(read_sensor(EVENTS.sender()).unwrap());
}
```

The init function is `pub` (or `pub(crate)`) and does not return a
`SensorReader` struct — embassy's design treats the runtime as the
holder of state, with channels and statics as the persistent surface.
**Do NOT** wrap the spawn calls in a `Result` from init; spawn failures
in embassy indicate a programming error (pool exhaustion, double-spawn)
that should panic, not propagate.

## Send + Sync rules

Anything held across an `.await` point in a task must be `Send` (the
embassy executor moves tasks across threads on multi-core targets, and
the `platform-cortex-m` executor still requires `Send` for soundness with
interrupt-driven wake-ups). Codegen guidelines:

- Task-local state held across awaits must be `Send`. Raw pointers,
  `Rc`, and `Cell<T>` typically aren't.
- Channel payloads must be `Send`. Use `Mutex<RawMutex, T>` from
  `embassy_sync` if you need shared mutable state.
- **Do NOT** hold a `core::cell::RefCell` or `core::cell::Cell` across
  an await — the borrow checker permits it but the resulting code is
  unsound on multi-core. Use `embassy_sync::mutex::Mutex` instead.

## Pool-size emission

`pool_size` drives both the task macro attribute and the spawn-site
constraints:

- `pool_size = 1` (default): task is spawnable once. Re-spawning panics.
- `pool_size = N` (N > 1): task is spawnable up to N times concurrently.
  The N spawn sites must each pass distinguishing arguments (typically
  an instance id or a peripheral handle).

When `pool_size > 1` and the task takes no parameters, codegen emits a
warning comment — pool_size > 1 with no parameters is almost always an
authoring mistake (every spawned instance does identical work).

## Interaction with module conventions

The firmware-layered profile's `module.conventions.md` says "actions
become methods on the module struct." embassy tasks are NOT actions in
that sense — they're long-lived async fns owned by the runtime, not
method calls on a value. Coexistence rules:

- A module may have BOTH `actions` (firmware-layered's convention) AND
  `embassy.tasks` (this block's convention). Actions are synchronous
  methods; tasks are async free functions. They don't conflict.
- A module's `init(spawner)` function (this block's convention)
  REPLACES firmware-layered's notion of a constructor when embassy is
  inlined. The module struct itself may not exist at all — embassy's
  data lives in statics + channels, not in struct fields.

**Do NOT** call task functions from synchronous action methods —
spawning is for init time only; cross-context calls go through channels.
