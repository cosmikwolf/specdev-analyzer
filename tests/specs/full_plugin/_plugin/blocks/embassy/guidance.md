# embassy — implementation guidance

Rules an agent follows when it writes or edits code for a module with an `[embassy]` table: the async runtime's
tasks, channels and spawning. No check verifies them; review does.

## Scope

- This block adds the executor, sync and time crates only. A peripheral HAL pairing belongs to a bridge.
- The panic handler and the critical-section implementation are project choices in `Cargo.toml`, never named in
  this block's code. A single-core Cortex-M project enables the core crate's single-core critical section, unless
  it picked another implementation — exactly one must link.
- With no allocator, channels and statics are the only persistent state.

## Tasks

- Each `[[embassy.tasks]]` entry is a free `async` function with the executor's task attribute — never a method in an
  `impl` block, which breaks spawning.
- `pool_size` goes into the task attribute. With `pool_size` above one, each spawn passes distinguishing arguments
  (an instance id, a peripheral handle); a pooled task with no parameters is almost always a mistake.
- Re-spawning a task past its pool size panics.

## Channels

- Each `[[embassy.channels]]` entry is one `static` at module scope, of the sync type that its `kind` names
  (`bounded`, `watch`, `signal`), with `payload_type` verbatim and `depth` as the size. The static's name is the
  channel name in upper case.
- Never a channel inside an `impl` block or as a struct field: channels need a `'static` lifetime.

## Spawning

- The module's `init`, taking the spawner, is the one spawn site; it replaces the profile's constructor, and the
  module struct may not exist at all.
- A spawn failure (pool exhausted, double spawn) is a programming error: it panics, it is not returned. Check where
  the executor version reports the failure — on the task call or on the spawn — and unwrap there.
- Synchronous action methods never call task functions; cross-context calls go through channels.

## Send and Sync

- State held across an `.await` must be `Send`, and channel payloads must be `Send`.
- Never hold a `Cell` or `RefCell` across an await; use the sync crate's mutex for shared mutable state.
