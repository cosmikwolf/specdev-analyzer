# Codegen — `rtic` block (Rust)

How to emit source from a module's `[rtic]` table. Every signature below is
taken from the pinned crate and compiled in this block's `reference/rust/`
crate; do not substitute a remembered form.

Read the project's profile's `module.conventions.md` for module-wide rules
(naming, error handling, doc comments). This file covers only what the
`[rtic]` table adds.

---

## 1. The application is one module, and one per crate

Everything a module's `[rtic]` table declares is emitted **inside a single
`#[rtic::app]` module**:

```rust
#[rtic::app(device = <device-path>, dispatchers = [<vector>, ...])]
mod app {
    // shared/local structs, init, and every task go here
}
```

**Do NOT emit two `#[rtic::app]` modules in one crate.** The macro generates an
entry point and a vector table, so a second app fails to compile. If a project
appears to need two, it needs two binaries.

**Do NOT invent the `device` path or the `dispatchers` list.** Both are facts
about the target chip: which path exposes the interrupt vocabulary, and which
vectors are free for the executor. This block does not know them — the project's
chip binding supplies them. Emit them from what that binding provides, and if
nothing does, stop and say so rather than guessing a vector name.

**One dispatcher is required per distinct software-task priority level.** Three
software tasks at priorities 1, 1 and 2 need two dispatchers. Fewer is a compile
error naming the shortfall (`not enough interrupts to dispatch all software
tasks`); more is harmless.

**The project must select rtic's backend feature, and this block cannot.**
`rtic` compiles only with exactly one backend feature, chosen by the core the
project targets (`thumbv7-backend` for Cortex-M3/M4/M7, `thumbv6-backend` for
M0/M0+, `thumbv8base-backend` / `thumbv8main-backend` for M23 / M33-M55-M85);
this block is core-agnostic and contributes the crate without one. The project
states it in `specs/architecture.toml`, where the manifest merge adds the
feature to the contributed crate:

```toml
[[external_deps]]
crate = "rtic"
version = "^2.3"
features = ["thumbv7-backend"]
```

Without it the build stops at `rtic-macros: No backend feature selected` —
if the entry is missing, add it (deriving the feature from
`[platform].target_triple`) rather than emitting a build that cannot
compile.

## 2. Resources — `[[rtic.resources]]`

Each entry becomes a field of one of two generated structs, selected by
`sharing`:

```rust
#[shared]
struct Shared {
    counter: u32,          // sharing = "shared", type = "u32"
}

#[local]
struct Local {
    toggle: bool,          // sharing = "local", type = "bool"
}
```

`init` returns both, and is where every resource gets its initial value:

```rust
#[init]
fn init(_cx: init::Context) -> (Shared, Local) {
    (Shared { counter: 0 }, Local { toggle: false })
}
```

**The access rule, which is the reason this block exists:**

| Declared | How a task reaches it | Why |
|---|---|---|
| `sharing = "shared"` | **always** `.lock(\|v\| …)` | more than one task can reach it |
| `sharing = "local"` | plain `&mut` via `cx.local.<name>` | no other task can name it |

```rust
// shared — the closure receives &mut, and the lock is not optional
cx.shared.counter.lock(|counter: &mut u32| *counter += 1);

// local — direct
*cx.local.toggle = !*cx.local.toggle;
```

**Do NOT emit a direct access to a `#[shared]` resource under any
circumstance.** There is no exception for the highest-priority accessor, for a
sole accessor, or for same-priority peers — all three were compiled and all
three are rejected. The error names the type
`<name>_that_needs_to_be_locked`, which is the framework telling you the fix.

**Do NOT declare a resource `shared` when one task uses it.** `local` is what
gives direct access; reaching for `shared` and then locking a resource nobody
contends for is pure overhead and misleads the next reader about the design.

### Locals that must not cross the init stack — declare them inline

A `Local` field is constructed in `init` and then moved into the framework's
storage, so a large value (a sample buffer, a step table) sits on the init
stack frame first. The framework accepts an inline-initialized local on `init`
and on any task; the value lives in a static from the start and the context
hands out a `&'static mut`:

```rust
// On init — the borrow can then be stored in a Shared or Local field, which
// is how a large table becomes lockable state without ever being on the stack.
#[init(local = [table: [u16; 64] = [0; 64]])]
fn init(cx: init::Context) -> (Shared, Local) {
    let table: &'static mut [u16; 64] = cx.local.table;
    (Shared { counter: 0, table }, Local { toggle: false })
}

// On a task — private to that task, same static placement.
#[task(binds = <binds>, local = [ticks: u32 = 0], priority = 3)]
fn sample(cx: sample::Context) {
    *cx.local.ticks += 1;
}
```

The initializer must be a constant expression. Prefer this placement for a
`sharing = "local"` resource whose type is large, or whose consumer needs a
`'static` borrow; use the `Local` struct form for everything else. Both are
`sharing = "local"` in the spec — the placement is an emission choice, not a
schema field.

**Do NOT reach for a `static mut` plus a raw-pointer cast to get a large
table into the app.** That is exactly the aliasing the resource model exists
to remove; the inline local gives the same static storage with a checked
borrow.

## 3. Tasks — `[[rtic.tasks]]`

`kind` decides the shape completely. **Never emit an `async` hardware task or a
non-`async` software task** — the framework rejects both, and `kind` is the only
input that decides it. There is deliberately no `async` field to consult.

### `kind = "hardware"`

Runs in the interrupt vector named by `binds`. A plain `fn`.

```rust
#[task(binds = <binds>, shared = [counter], local = [toggle], priority = 3)]
fn sample(mut cx: sample::Context) {
    cx.shared.counter.lock(|counter: &mut u32| *counter += 1);
    *cx.local.toggle = !*cx.local.toggle;
}
```

**Do NOT `.await` anywhere in a hardware task**, and do not reach for a
block-on helper to get around it — that reintroduces the priority inversion the
resource model exists to remove. Work that must await belongs in a software
task; hand it over by spawning one or sending on a channel's sync path (§4).

### `kind = "software"`

Runs on the executor in a dispatcher vector. Always an `async fn`.

```rust
#[task(shared = [counter], priority = 1)]
async fn drain(mut cx: drain::Context) {
    let value = cx.shared.counter.lock(|counter: &mut u32| *counter);
    let _ = value;
}
```

Start one from `init` or from another task through its generated `spawn`:

```rust
drain::spawn().ok();
```

**Do NOT `.unwrap()` a `spawn()`.** It returns `Err` when an instance is already
pending, which is a normal runtime state and not a fault.

### `kind = "idle"`

Runs when nothing else is runnable, never returns, takes no `priority`.

```rust
#[idle]
fn idle(_cx: idle::Context) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
```

**Do NOT emit more than one `#[idle]`**, and do not emit one at all unless the
spec declares it — the framework supplies its own default otherwise.

## 4. Channels — `[[rtic.channels]]`

A channel entry says **what** flows and **how many** slots. **Which crate**
carries it is the project's choice, and this block contributes neither
candidate. Read the project's manifest (`[[external_deps]]` in
`specs/architecture.toml`) and emit the form for the crate it holds:

| Manifest holds | Emit |
|---|---|
| `rtic-sync` | form A — `make_channel!` in `init`, ends handed to tasks as locals |
| `embassy-sync` | form B — a `static Channel<…>` at module scope |
| neither | **stop** and say so; a channel crate is a dependency the project must accept knowingly |
| both | **stop** and say so; one crate carries every channel the app declares |

Both forms compile in this block's reference crate.

### Form A — `rtic-sync`: created in `init`, ends as locals

```rust
use rtic_sync::{channel::{Receiver, Sender}, make_channel};

let (tx, rx) = make_channel!(u32, 4);      // payload_type, capacity
```

The ends are typed `Sender<'static, u32, 4>` and `Receiver<'static, u32, 4>` —
the capacity is part of the type, so it must match the declaration exactly:

```rust
#[task(local = [tx, rx], priority = 2)]
async fn pump(cx: pump::Context) {
    let tx: &mut Sender<'static, u32, 4> = cx.local.tx;
    let rx: &mut Receiver<'static, u32, 4> = cx.local.rx;

    tx.send(1).await.ok();
    if let Ok(value) = rx.recv().await {
        let _ = value;
    }
}
```

**Do NOT use a form-A channel from a hardware task** — both ends `.await`.
A hardware task that must emit an event uses form B's sync send, or latches
into a shared resource and spawns a software task.

### Form B — `embassy-sync`: a `static` at module scope

```rust
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;

// [[rtic.channels]] name = "events", payload_type = "u32", capacity = 4
static EVENTS: Channel<CriticalSectionRawMutex, u32, 4> = Channel::new();
```

The static name uppercases the channel name (`events` → `EVENTS`). It is
reachable from every task without passing ends through resources, and it has
a sync path on both sides, which is what lets a **hardware** task feed a
**software** task:

```rust
// hardware task — sync send; Err means the channel is full, which is a
// runtime state to count or drop, never to unwrap
#[task(binds = <binds>, priority = 3)]
fn sample(_cx: sample::Context) {
    if EVENTS.try_send(1).is_err() {
        // full: record the overflow and move on
    }
}

// software task — awaits
#[task(priority = 1)]
async fn drain(_cx: drain::Context) {
    let value = EVENTS.receive().await;
    let _ = value;
}
```

`try_receive` is the sync receive for the rare hardware-task consumer. The
`CriticalSectionRawMutex` parameter needs a `critical-section` implementation
at link time — on single-core Cortex-M that is the `cortex-m` crate's
`critical-section-single-core` feature, which the project's platform pick
normally already enables; if the link fails on `_critical_section_1_0_acquire`,
that feature is what is missing.

**Do NOT `.await` a form-B channel in a hardware task.** `receive()` and
`send()` are futures; the hardware side uses `try_send` / `try_receive` only.

**Do NOT place the static inside `init`.** It is `'static` by construction and
lives at module scope (inside `mod app` is fine).

### Both forms

**Do NOT use a channel where a shared resource is meant.** A channel carries
values from one task to another; a shared resource is state a task mutates in
place. Emitting a channel for state produces a queue nobody drains.

**Do NOT mix forms in one application.** One crate carries every channel the
app declares; if the manifest holds both, stop and say so.

## 5. Error handling

Task bodies return `()`; there is no error channel out of a task. A fallible
operation is handled inside the task — log it, latch it into a resource, or
send it on a channel to a task that owns the reporting. **Do NOT `panic!` or
`.unwrap()` on a recoverable error inside a task**: a panic in an interrupt
context takes the whole application down with no way to observe why.
