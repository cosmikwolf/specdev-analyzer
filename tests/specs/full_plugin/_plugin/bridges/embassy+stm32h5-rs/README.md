# `embassy+stm32h5-rs` bridge

A canonical bridge wiring the `embassy` async runtime block to the
`stm32h5-rs` HAL block. Inline this bridge when a project's modules
own both `[embassy.tasks]` and `[stm32h5.peripherals]` and need a
declared handoff between them.

## When to use

Inline this bridge when:

- The project uses the `firmware-layered` profile (or any profile that
  exposes `Module`).
- Both `embassy` and `stm32h5-rs` are inlined in the project.
- One or more modules declare embassy tasks AND stm32h5-rs peripherals
  that the tasks should consume directly.

Skip this bridge when:

- A module has embassy tasks and stm32h5-rs peripherals but the tasks
  use channels for communication only (no peripheral handle handoff
  needed).
- The project uses `embassy-stm32` for async peripheral access. The
  `stm32h5-rs` block targets the non-embassy `stm32h5xx-hal`; pairing
  embassy with `embassy-stm32` is a different bridge (deferred — open a
  feature request).

## What this bridge contributes

**Bindings shape** (under `bindings."embassy+stm32h5-rs"` on `Module`):
`peripheral_tasks: [{task, peripheral, handoff}]`. See
`bindings.schema.yaml` for the full schema.

**Reduction relations**: one `peripheral_owned_by` relation per
binding entry, from `stm32h5-rs.Peripheral` to `embassy.Task`. Extends
canonical `depends_on` so layer-dependency / acyclic / reachability
graph checks work with zero domain knowledge.

**Guidance** (`guidance.md`): two patterns —
- `handoff = "owned"` — peripheral moves into the spawned task.
- `handoff = "shared"` — peripheral lives in a `Mutex<RawMutex, T>`
  static; multiple tasks acquire `.lock().await` to use it.

## What this bridge does NOT contribute

- **No join-product crate.** The code uses the crates the two blocks'
  guidance covers (`embassy-executor`, `embassy-sync`, `embassy-time`,
  `stm32h5xx-hal`, `stm32h5xx-pac`); the project's `Cargo.toml` declares
  them. The bridge is purely a wiring discipline.
- **No interrupt-driven async.** That requires `embassy-stm32`, a
  different HAL. Future variant.
- **No cross-module bindings.** Both endpoints must be declared on the
  same module spec.

## Inline order

`/spec-inline-bridge embassy+stm32h5-rs` requires both blocks to be
inlined first (`INLINE-003` check). Order:

```
/spec-inline embassy
/spec-inline stm32h5-rs
/spec-inline-bridge embassy+stm32h5-rs
```

Lex order (`embassy < stm32h5-rs`) is enforced (`BRIDGE-LEX-001`); use
exactly this name.
