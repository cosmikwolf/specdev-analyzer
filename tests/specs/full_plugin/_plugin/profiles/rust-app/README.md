# rust-app profile

A profile for standalone Rust applications — a desktop app, a CLI, a
service — built as a **testable core behind `src/lib.rs` plus a thin
`src/main.rs` entry point**, in one crate.

## When to use this profile

Use `rust-app` for projects that:

- Build an executable that is not firmware and not hosted by a plugin
  host — the app owns its own `main`.
- Want their logic testable without launching the binary. The `[lib]`
  target is where the specs live; `cargo test` exercises it directly.
- Have a layering worth stating and enforcing (entry / adapter / core /
  support), rather than a flat set of modules.
- Depend on ordinary crates.io crates and use cargo features to gate
  optional functionality.

Don't use this profile if:

- The crate is consumed by *other* crates and its public API is the
  product — that's `library-api`, where the API surface, re-export
  discipline and semver are the load-bearing concerns.
- The target is embedded firmware with a hardware surface (pins,
  peripherals, ISRs) — that's `firmware-layered`.
- A host supplies the window and the event loop — that's
  `audio-plugin`.
- The project is several crates. Use `rust-workspace` at the root and
  give each member its own profile; `rust-app` is a fine choice for a
  binary member.

## Why one crate with two targets (RUST-APP-LIB-PLUS-BIN)

The obvious alternative — a workspace with a `core` library member and a
`gui` binary member — is a heavier default than most applications need,
and it is easy to adopt later. One crate with `src/lib.rs` +
`src/main.rs` gets the property that actually matters (logic testable
without starting the process) at zero structural cost:

1. **`cargo test` already does the right thing.** Tests link the lib
   target; nothing has to start, bind a port, or open a window.
2. **The split costs one line.** `main.rs` calls into the lib and
   returns. There is no dependency edge to declare, no second spec tree,
   no version to keep synced between two crates.
3. **Splitting later is mechanical.** When a second consumer of the core
   appears, `/spec-add-member` adopts this crate into a workspace with
   its requirement and decision ids intact. Splitting *before* that
   consumer exists buys nothing and costs a boundary you must maintain.

The practical rule this profile enforces: **codegen never writes
`src/main.rs`.** It is emitted once from a stub template and is yours
thereafter. Startup logic that deserves a test goes in an `entry`-layer
module under the lib, and `main` calls it.

## Layers

Modules declare a `layer`; the `layer_dependency` graph check enforces
downward-only dependencies (same-layer is allowed, upward is an error).

| layer | holds | rule |
| --- | --- | --- |
| `entry` | startup wiring, config and argument parsing, building the object graph | nothing here should be worth unit-testing on its own |
| `adapter` | everything touching the outside world: network, filesystem, database, GUI, clock, other processes | a real implementation and a test double should both be plausible |
| `core` | the application's own types and rules | no I/O, no framework imports — testable with no runtime |
| `support` | error types, logging setup, small shared helpers | keep it small; a growing `support` usually means something belongs in `core` |

`core` importing nothing is the load-bearing constraint — it's what makes
the application testable at all. When `core` needs the outside world,
declare a trait in `core`, implement it in `adapter`, and wire the two in
`entry`.

## Profile contents

| File | Role |
|---|---|
| `schemas/module.schema.yaml` | per-module spec shape (layer, visibility, items[], feature_gates) |
| `schemas/architecture.schema.yaml` | `specs/architecture.toml` shape (cargo features, external deps) |
| `schemas/requirements.schema.yaml` | `specs/requirements.toml` shape (sources + reqs) |
| `schemas/enums.schema.yaml` | shared closed vocabularies (`Layer`, `Visibility`, `FeatureName`) |
| `reductions/project.yaml` | TOML → universal entity/relation graph |
| `codegen/rust/module.conventions.md` | Rust emission conventions for applications |
| `templates/main-stub.rs.template` | first-emit `src/main.rs` body; never overwritten |
| `spec.toml.template` | bootstrap-time `spec.toml` skeleton |
| `domain.toml.template` | bootstrap-time `domain.toml` skeleton + graph checks |
| `architecture.toml.template` | bootstrap-time features/deps file with examples |
| `requirements.toml.template` | bootstrap-time requirements file with examples |

## Module shape

A `rust-app` Module is a unit of the application, placed on the layer
axis:

```toml
name = "PeerSession"
layer = "core"
visibility = "pub"
description = "Tracks the state of one connected peer."
depends_on = ["WireFormat"]
satisfies = ["REQ-NET-002"]

[[items]]
name = "PeerSession::apply"
kind = "fn"
description = "Applies an inbound frame, returning the resulting state change."
visibility = "pub"
```

This produces `src/peer_session.rs`, declared from `src/lib.rs` as
`pub mod peer_session;`.

## Cargo.toml emission

The manifest renders `[lib]` and `[[bin]]` together:

```toml
[lib]

[[bin]]
name = "my-app"
path = "src/main.rs"
```

`[[bin]]` comes from the profile manifest's `cargo_toml.bin` entry; the
crate name drives both the package name and the binary name. Cargo
features declared in `architecture.toml` render as usual, with the
synthetic `default = [...]` line.

## Composing with blocks

`rust-app` deliberately has no framework surface, which makes it the
neutral host for blocks. A desktop GUI app inlines the blocks for its
stack (window/shell, UI topology, transport); each contributes its own
namespaced table on the modules that need it, and its own cargo deps.
The profile itself stays out of the way.
