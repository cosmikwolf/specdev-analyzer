# `flutter-rust-bridge` block

Models the Dart↔Rust FFI seam of a Flutter app that consumes a companion
Rust crate through [flutter_rust_bridge](https://github.com/fzyzcjy/flutter_rust_bridge)
(FRB) v2. Contributes a `[flutter_rust_bridge]` table to `Module` plus the
codegen prose that emits the FRB config file and sequences the standalone
generator CLI, Cargokit integration, and web/wasm builds.

## Platform status — read before choosing this block

Verified working: **web**, **host/desktop** (including the companion crate
compiling for `wasm32-unknown-unknown`), and **Android**.

Android needs **two mandatory steps** that `integrate` does not do for you.
Skip either one and the build still reports success while the `.apk`
contains no Rust — see "Android: two mandatory steps" in the codegen prose
for the commands, and "Assert the artifact, not the exit code" below for
why a green build is not evidence.

1. **Replace the vendored Cargokit with the maintained fork.** FRB 2.12.0's
   `integrate` vendors a copy taken from the now-**archived** `irondash`
   repository. That copy calls Gradle's `Project.exec()`, removed in
   Gradle 9, so the build fails loudly with `Could not find method exec()`.
   Current Flutter ships Gradle 9.x and the Android Gradle Plugin it ships
   refuses to run on Gradle 8, so pinning cannot satisfy both. The
   maintained fork (`fzyzcjy/cargokit`, same author as FRB) already uses
   the injected `ExecOperations`, and clears this.
2. **Name the companion crate with underscores, not hyphens.** Cargokit
   derives the artifact filename from the crate's `[package].name`
   *verbatim* (`build_tool/lib/src/artifacts_provider.dart`,
   `getArtifactNames` → `'lib$libraryName.so'`), while cargo normalizes
   `-` to `_` when it writes the file. A crate named `hello-rust` makes
   Cargokit look for `libhello-rust.so`; cargo wrote `libhello_rust.so`;
   the existence filter drops it, the `jniLibs/<buildType>/<abi>/` staging
   directories are created and left **empty**, and the source set Cargokit
   registers contributes nothing. This is a spec-owned fact: set
   `[project].name` in the companion crate's `spec.toml`, so
   `manifest --write` keeps it. Note that `bootstrap --name` documents
   itself as "lowercase, hyphens" — correct for a normal crate, wrong for
   an FRB companion.

### Assert the artifact, not the exit code

Both failure modes above are **silent**: `flutter build apk` exits 0 and
the app crashes at `RustLib.init()` on a device. A gate over the `.apk`
must assert the library is *inside* it, not that the command succeeded:

```sh
unzip -l build/app/outputs/flutter-apk/app-debug.apk | grep 'lib<crate_name>.so'
```

A debug `.apk` carries the ABIs Flutter selects (typically `arm64-v8a`,
`armeabi-v7a`, `x86_64`); Cargokit stages four, including `x86`.

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
