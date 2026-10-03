# rtic — implementation guidance

Rules an agent follows when it writes or edits code for a module with an `[rtic]` table. No check verifies them
yet; review does.

## The application

- All that a module's `[rtic]` table declares lives in one `#[rtic::app]` module. A crate has one app; two apps
  need two binaries.
- The app's `device` path and `dispatchers` list are facts about the chip. Take them from the project's chip unit;
  if nothing gives them, stop and ask. Do not guess a vector name.
- Give one dispatcher for each distinct software-task priority level.
- rtic compiles only with exactly one backend feature, chosen by the target core. The project's `Cargo.toml`
  selects it; this block cannot.

## Resources

- A `sharing = "shared"` resource is always reached through its lock. There is no exception: not for the
  highest-priority user, a sole user, or same-priority peers.
- A resource that one task uses is `sharing = "local"`, reached directly. Do not declare it shared and lock it.
- `init` gives every resource its first value.
- A large local value, or one that needs a `'static` borrow, goes in an inline-initialized local on `init` or on
  the task, so it never sits on the init stack. Never use a `static mut` with a raw-pointer cast instead.

## Tasks

- `kind` alone decides the shape: a hardware task is a plain function bound to the `binds` vector; a software task
  is an `async` function on the executor; `idle` never returns and has no priority.
- A hardware task never awaits, and never uses a block-on helper. Work that must await goes to a software task.
- A `spawn` can fail because an instance is already pending — a normal state. Do not unwrap it.
- At most one `#[idle]`, and only when the spec declares one.
- The task names, `binds`, `priority` and the resources each task uses match the spec's `[[rtic.tasks]]` entries;
  `uses` lists rtic resources only (hardware handles of the chip unit can share the `#[local]` struct).

## Channels

- A channel carries values between tasks; state that a task changes in place is a shared resource, not a channel.
- Which crate carries channels is the project's choice in `Cargo.toml`. If it holds neither candidate, or both, stop
  and ask. One crate carries every channel in the app.
- A channel whose ends both await is for software tasks only. A hardware task that must emit an event uses a channel
  with a non-blocking send, or latches into a shared resource and spawns a software task.
- A full channel on a non-blocking send is a runtime state: count or drop, never unwrap.
- The channel's capacity and payload type match the `[[rtic.channels]]` entry.

## Errors

- A task returns nothing. Handle a fallible operation inside the task: log it, latch it into a resource, or send it
  to the task that reports it.
- Never panic or unwrap on a recoverable error in a task: a panic in an interrupt context stops the whole
  application with no trace.
