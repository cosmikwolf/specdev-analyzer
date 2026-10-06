# flutter-rust-bridge — implementation guidance

Rules an agent follows when it writes or edits code for a module with a `[flutter_rust_bridge]` table: a Flutter app
calling a companion Rust crate through flutter_rust_bridge (FRB). No check verifies them; review does.

## Versions and config

- The FRB pub package and FRB's generator CLI (`flutter_rust_bridge_codegen`) run at the same exact version; FRB refuses to run otherwise. Pin it
  exactly, never as a range.
- `flutter_rust_bridge.yaml` at the app root matches the table: `rust_input` (comma-joined), `rust_root`,
  `dart_output` (default `lib/src/rust`), and `no_web: true` only when `web = false`. No keys the spec does not model.

## Build steps

- FRB's generator is a standalone CLI, not a build_runner step, run from the app root, again whenever the scanned
  Rust modules change. `integrate` runs once, with the options that stop it writing `lib/` and an integration test;
  its platform hooks are user action items. Afterwards, restore the spec's config if `integrate` replaced it.
- Web builds compile the crate to wasm before the Flutter web build.
- Android: replace the vendored, archived Cargokit with the maintained fork, and give the companion crate an
  underscored package name (Cargokit looks for the library under the hyphenated name). Either miss gives a green build
  with no Rust in the APK, so check the artifact for the library, not the exit code.
- `integrate` adds a path dependency on the Cargokit shim to `pubspec.yaml`; keep it.

## The companion crate

- Its crate root declares the generated module itself. Otherwise `generate` inserts the line at the top of the file,
  above any module doc comment, which does not compile.
- On the web it stays wasm-pure: no OS threads (use FRB's spawn), a minimal async runtime, no unwind catching, no
  64-bit typed lists across the boundary. Threaded web builds need cross-origin isolation headers.
- Transport (devices, sockets, permissions) stays on the Dart side; the crate processes bytes it is given.

## Using the bindings

- The app awaits FRB's library init before it runs the app.
- Sync Rust functions become plain Dart functions; others return futures; a Rust function with a stream sink returns a
  Dart stream.
- The wrapping module is the only importer of generated files; the spec's items describe the wrapper's API, never
  the generated surface.
- Generated files (`dart_output`, the crate's generated Rust, the integration hooks) are never hand-edited.
