# `rtic` block

Models a Real-Time Interrupt-driven Concurrency application: tasks bound to
interrupt priorities, the shared and local resources they access, and the
channels between them.

Author-facing. This file is **not** inlined into projects.

## When to use it

Reach for this block when a project's concurrency needs *priorities and
preemption* — an interrupt that must not be delayed by lower-priority work, or
state touched from more than one priority level. That second half is the real
reason: the framework's contribution is that shared state is accessed under a
lock whose ceiling the compiler derives, replacing hand-rolled critical
sections that are easy to get subtly wrong.

Do not reach for it merely to run several things at once. A cooperative async
runtime is simpler and does that well. The question that selects this block is
"does something need to preempt something else, and do they share state?"

## What it contributes

| Table | Becomes |
|---|---|
| `[[rtic.tasks]]` | `#[task]` fns inside the `#[rtic::app]` module |
| `[[rtic.resources]]` | fields of the generated `Shared` / `Local` structs, or an inline-initialized local when the value must not cross the init stack |
| `[[rtic.channels]]` | one of two forms, chosen by the channel crate the project's manifest holds — see below |

Crates: `rtic` only. See `codegen/rust/deps.yaml` — note it declares **no
backend feature**, because the correct one depends on the target core, and
**no channel crate**, because that is the project's choice (next section).

## Channels: the crate is the project's choice

`[[rtic.channels]]` says what flows and how many slots. Two crates can carry
it, and they suit different projects:

| Manifest holds | Emitted form | Suits |
|---|---|---|
| `rtic-sync` | `make_channel!` in `init`, `Sender` / `Receiver` ends handed to tasks as locals | a project with no async-sync crate yet |
| `embassy-sync` | a `static Channel<CriticalSectionRawMutex, T, N>` at module scope | a project that already runs `embassy-sync` — for instance one that keeps `embassy-time` or an async shared-bus under this framework, which is a real and maintained pairing |

Both are `recommended_external_deps`; neither is contributed. A manifest
contribution cannot be conditional, so contributing one crate would force it
on the projects that already run the other. The codegen prose documents both
forms in one place, keyed on a feature check (which crate is present), and
the reference crate compiles both.

**History, for downstream projects.** Until 2026-09-01 this block contributed
`rtic-sync` unconditionally and documented only that form. A project that
inlined the block *for that crate* and re-inlines now will stop receiving it
at the next `manifest --write`; add `rtic-sync` to the project's own
`[[external_deps]]` to keep it. Nothing else about the block changed shape:
no field, table, entity, or relation was removed or renamed.

## What it deliberately does not model

- **`async` on a task.** Fully determined by `kind`: software tasks are always
  `async fn`, hardware tasks always `fn`. A field could only agree or
  contradict, so codegen derives it.
- **Whether an access needs a lock.** Determined by the resource's `sharing`.
  Recording it per-access would be a second source of truth for something the
  framework already decides — and enforces at compile time.
- **Which channel crate.** A manifest fact, read at emission time. A field
  here would duplicate the manifest and could contradict it.
- **Where a local lives.** `sharing = "local"` covers both a `Local` struct
  field and an inline-initialized local; the placement is an emission choice
  driven by the value's size and whether a consumer needs a `'static` borrow.
- **Anything about a chip.** Which vectors exist, which are free for the
  executor's dispatchers, and what path exposes them are all chip facts. They
  arrive from the chip binding.

## Pairing

This block cannot build from its own contributions alone, by construction: an
application needs a `device` path and a dispatcher vector list, and both are
chip facts. Pair it with the binding for your chip family, which supplies them
as a join.

That binding is also where the dispatcher count is decided, and the rule is
worth knowing while sizing it: **one dispatcher vector per distinct
software-task priority level.**

## Verification status

The codegen prose has a compiled witness: `reference/rust/` builds the worked
examples against the pinned API in plugin CI, with `unused_crate_dependencies`
on so a contributed-but-unexercised crate fails.

**One case, not one per kind.** `#[rtic::app]` generates an entry point and a
vector table, so two applications in one crate collide and rustc refuses them.
The application is the framework's emission unit, so it is this block's
reference unit too; every kind is exercised inside the single app — including
both channel forms and the inline-local placement.

**The reference carries crates this block does not contribute.** `rtic-sync`
and `embassy-sync` appear in `reference/rust/Cargo.toml` as witnesses for the
two channel forms. A recommended crate whose form nobody compiles is a form
nobody has checked.

Three claims in the prose were **corrected by that reference on its first
build**, and are recorded here because each is easy to get wrong from memory:

1. A `#[shared]` resource needs `.lock()` **always** — not merely below the
   ceiling. The highest-priority accessor, a sole accessor, and same-priority
   peers were each compiled, and all three are rejected without a lock.
2. The number of dispatchers must be at least the number of **distinct
   software-task priority levels**, not one.
3. Only `#[local]` gives direct access. That is what makes `sharing` a real
   design decision rather than bookkeeping.

## Deferred

- **Monotonics** (`spawn_after` / delays). A project that already has a time
  source keeps it; adding a second time stack is a decision worth making
  explicitly rather than by default. Promote when a project needs it.
- **Multi-instance tasks.** The framework's capacity model differs from a task
  pool; not modelled until a project needs it.
- **An initial-value field on resources.** Today the emitter derives initial
  values from the module's description and items, as it does for `Local`
  fields. A typed `init` expression would make the inline-local placement
  fully spec-driven; promote when an emitter gets it wrong.
