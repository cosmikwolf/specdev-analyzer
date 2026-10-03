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

| Table | In the code |
|---|---|
| `[[rtic.tasks]]` | `#[task]` fns inside the `#[rtic::app]` module |
| `[[rtic.resources]]` | fields of the `Shared` / `Local` structs, or an inline-initialized local when the value must not cross the init stack |
| `[[rtic.channels]]` | one of two forms, chosen by the channel crate the project's manifest holds — see below |

The guidance (`guidance.md`) carries the rules: the lock model, task shapes, channel forms, error handling. The
project's `Cargo.toml` selects rtic's backend feature — the correct one depends on the target core — and the channel
crate.

## Channels: the crate is the project's choice

`[[rtic.channels]]` says what flows and how many slots. Two crates can carry it, and they suit different projects:

| Manifest holds | Form | Suits |
|---|---|---|
| `rtic-sync` | `make_channel!` in `init`, `Sender` / `Receiver` ends handed to tasks as locals | a project with no async-sync crate yet |
| `embassy-sync` | a `static Channel<CriticalSectionRawMutex, T, N>` at module scope | a project that already runs `embassy-sync` — for instance one that keeps `embassy-time` or an async shared-bus under this framework, which is a real and maintained pairing |

The guidance states the rules for both forms, keyed on which crate the manifest holds.

## What it deliberately does not model

- **`async` on a task.** Fully determined by `kind`: software tasks are always
  `async fn`, hardware tasks always `fn`. A field could only agree or
  contradict.
- **Whether an access needs a lock.** Determined by the resource's `sharing`.
  Recording it per-access would be a second source of truth for something the
  framework already decides — and enforces at compile time.
- **Which channel crate.** A manifest fact. A field here would duplicate the
  manifest and could contradict it.
- **Where a local lives.** `sharing = "local"` covers both a `Local` struct
  field and an inline-initialized local; the placement is a code choice
  driven by the value's size and whether a consumer needs a `'static` borrow.
- **Anything about a chip.** Which vectors exist, which are free for the
  executor's dispatchers, and what path exposes them are all chip facts. They
  arrive from the chip binding.

## Pairing

An application needs a `device` path and a dispatcher vector list, and both are
chip facts. Pair it with the binding for your chip family, which supplies them
as a join.

That binding is also where the dispatcher count is decided, and the rule is
worth knowing while sizing it: **one dispatcher vector per distinct
software-task priority level.**

## Checks

The task names, `binds`, `priority` and resources in `[[rtic.tasks]]` are the facts a spec ↔ code check reads from
`#[task(...)]` attributes (the attribute arguments are separate syntax nodes a query can read).
Until that check ships as a rule pack, review holds the code to them. A `uses` list names rtic resources only; the
chip's hardware handles can share the `#[local]` struct.

## Deferred

- **Monotonics** (`spawn_after` / delays). A project that already has a time
  source keeps it; adding a second time stack is a decision worth making
  explicitly rather than by default. Promote when a project needs it.
- **Multi-instance tasks.** The framework's capacity model differs from a task
  pool; not modelled until a project needs it.
- **An initial-value field on resources.** Initial values are a code fact
  today. Promote only if a check needs them.
