# `flutter-rust-bridge` block

Models the Dart↔Rust FFI seam of a Flutter app that consumes a companion
Rust crate through [flutter_rust_bridge](https://github.com/fzyzcjy/flutter_rust_bridge)
(FRB) v2. Contributes a `[flutter_rust_bridge]` table to `Module` plus the
implementation guidance for the FRB config file, the standalone generator
CLI, Cargokit integration, and web/wasm builds.

## Platform status — read before choosing this block

Verified working: **web**, **host/desktop** (including the companion crate
compiling for `wasm32-unknown-unknown`), and **Android**.

Android needs **two mandatory steps** that `integrate` does not do for you.
Skip either one and the build still reports success while the `.apk`
contains no Rust — the guidance names both steps, and "Assert the artifact, not the exit code" below for
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
  config singleton the FRB config file must match; the cross-project path
  is build-layer config, so there is nothing for the graph to check — the
  drift tripwire is FRB's own generator run.
- **Guidance:** the exact-version lock between the FRB CLI and the pub
  package, the config file, generator/integrate/build-web sequencing, the
  two Android steps, the four consumption shapes (init, sync call, async
  call, stream), generated-file ownership, and the wasm-purity rules for
  web-enabled apps. The project's `pubspec.yaml` pins `flutter_rust_bridge`
  **exactly**.

## What this block does NOT contribute

- **An API mirror.** The table never re-declares the exposed Rust
  symbols; the companion crate's own spec owns its API surface, and the
  generated bindings are the single binding-surface artifact. Listing
  symbols on the app side would be a second source of truth that drifts
  by construction.
- **Cross-project validation.** `rust_root` is unchecked at spec time —
  a wrong path or a drifted API fails at
  `flutter_rust_bridge_codegen generate`, which is the intended tripwire.
- **The companion crate's spec.** The companion crate is spec'd in its
  own project; FRB parses its plain source, so nothing leaks into it.

## Sharp-test rationale

- **Concreteness:** names one specific binding generator
  (flutter_rust_bridge v2), not "FFI" in the abstract.
- **Rule-pack completeness:** each field is an FRB config input the
  config file must match; the guidance carries the sequencing and
  ownership rules no check can verify.
- **No cross-block reach:** the guidance names no other block; the seam
  stands alone on any profile exposing `Module`.

## Companion affinity and deferred items

- Pairs naturally with the `universal_ble` block in device-companion
  apps: BLE transport stays Dart-side; received bytes cross the FRB seam
  for pure-Rust processing. No bridge exists between them — they share
  no spec-level surface.
- **Deferred:** modeling FRB as a true bridge (an `ffi_boundary`
  locality) becomes the right shape only if app and companion crate ever
  share one spec root; multi-crate FFI (several `[flutter_rust_bridge]`
  modules per app) is deferred until a real app needs it.
