# `flutter-rust-bridge` block

Models the Dart↔Rust FFI seam of a Flutter app that consumes a companion
Rust crate through [flutter_rust_bridge](https://github.com/fzyzcjy/flutter_rust_bridge)
(FRB) v2. Contributes a `[flutter_rust_bridge]` table to `Module` plus the
codegen prose that emits the FRB config file and sequences the standalone
generator CLI, Cargokit integration, and web/wasm builds.

## When to use

A Flutter app project needs to call Rust — DSP, protocol parsing, domain
logic — from Dart, on native platforms and (optionally) the web. The Rust
code lives in its **own project** (its own spec root, under whatever
Rust-shaped profile fits it); this block goes in the **app** project and
points at that crate. One module carries the table in v1: the service
that owns the FFI seam and wraps the generated bindings for the rest of
the app.

## What this block contributes

- **Schema:** the `[flutter_rust_bridge]` table — `rust_root` (path to
  the companion crate), `rust_input` (which of its modules FRB scans),
  `web` (wasm target on/off, required), optional `dart_output`. FRB
  config inputs only.
- **Reduction:** deliberately empty (`entities: []`). The table is a
  config singleton read by codegen prose; the cross-project path is
  build-layer config, so there is nothing for the graph to check — the
  drift tripwire is the FRB codegen run itself.
- **Deps:** `flutter_rust_bridge` pinned **exactly** (CLI and runtime
  versions must match; see `codegen/dart/deps.yaml` for the bump
  procedure).
- **Codegen prose:** config-file emission, generator/integrate/build-web
  sequencing, worked examples for the four consumption shapes (init,
  sync call, async call, stream), the generated-file ownership table,
  and the wasm-purity rules for web-enabled apps.

## What this block does NOT contribute

- **An API mirror.** The table never re-declares the exposed Rust
  symbols; the companion crate's own spec owns its API surface, and the
  generated bindings are the single binding-surface artifact. Listing
  symbols on the app side would be a second source of truth that drifts
  by construction.
- **Cross-project validation.** `rust_root` is unchecked at spec time —
  a wrong path or a drifted API fails at
  `flutter_rust_bridge_codegen generate`, which is the intended tripwire.
- **Rust-side codegen.** The companion crate is spec'd and generated in
  its own project; FRB parses its plain source, so nothing leaks into it.

## Sharp-test rationale

- **Concreteness:** names one specific binding generator
  (flutter_rust_bridge v2), not "FFI" in the abstract.
- **Codegen-completeness:** conforming TOML yields a working seam — the
  emitted config file drives a real generator run, and the prose's
  consumption idioms compile against the generator's output (see
  `reference/`).
- **No cross-block reach:** the prose names no other block; the seam
  stands alone on any profile exposing `Module`.

## Reference pair (CI-only, never inlined)

`reference/rust/` is a real companion crate (sync + async + stream API)
and `reference/dart/` a real consumer package whose committed bindings
were produced by the pinned generator. CI gates: `flutter pub get &&
flutter analyze` on the Dart side; `cargo build` on the Rust side pinned
to `wasm32-unknown-unknown` (the wasm-purity witness; the host build is
exercised at authoring time). Regenerating the bindings is part of any
version-bump change.

## Companion affinity and deferred items

- Pairs naturally with the `universal_ble` block in device-companion
  apps: BLE transport stays Dart-side; received bytes cross the FRB seam
  for pure-Rust processing. No bridge exists between them — they share
  no spec-level surface.
- **Deferred:** modeling FRB as a true bridge (an `ffi_boundary`
  locality) becomes the right shape only if app and companion crate ever
  share one spec root; multi-crate FFI (several `[flutter_rust_bridge]`
  modules per app) is deferred until a real app needs it.
