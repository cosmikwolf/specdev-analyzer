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
task; hand it over by spawning one or sending on a channel.

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

Created in `init` and handed to tasks as local resources:

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

**Do NOT use a channel from a hardware task** — both ends `.await`.

**Do NOT use a channel where a shared resource is meant.** A channel carries
values from one task to another; a shared resource is state a task mutates in
place. Emitting a channel for state produces a queue nobody drains.

## 5. Error handling

Task bodies return `()`; there is no error channel out of a task. A fallible
operation is handled inside the task — log it, latch it into a resource, or
send it on a channel to a task that owns the reporting. **Do NOT `panic!` or
`.unwrap()` on a recoverable error inside a task**: a panic in an interrupt
context takes the whole application down with no way to observe why.
