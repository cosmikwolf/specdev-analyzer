# flutter-rust-bridge — Dart codegen guidance

How a `[flutter_rust_bridge]` table on a module becomes working code. The
block owns exactly two emissions: the FRB config file and the Dart code
that consumes the generated bindings. Everything the generator itself
writes is generator-owned — codegen orchestrates it, never reproduces it.

## Package dependency and version lock

The block contributes `flutter_rust_bridge` at an **exact** version (see
`deps.yaml`). FRB refuses to run when the `flutter_rust_bridge_codegen`
CLI version differs from the pub package version, so the two move in
lockstep:

```sh
cargo install 'flutter_rust_bridge_codegen@2.12.0'
```

- Do **NOT** relax the pin to a caret range: the committed generated
  files were produced by exactly that CLI, and a solver-chosen newer
  runtime will fail FRB's version check.

## Emitting the FRB config file (spec-owned)

Emit `flutter_rust_bridge.yaml` at the app project root, deterministically
from the module's `[flutter_rust_bridge]` table. This file **is** codegen
output — record it in gen-lineage like any other emitted file.

For a table

```toml
[flutter_rust_bridge]
rust_root = "../rust/my_lib"
rust_input = ["crate::api"]
web = true
```

emit

```yaml
rust_input: crate::api
rust_root: ../rust/my_lib
dart_output: lib/src/rust
```

- Emit keys in exactly this order: `rust_input`, `rust_root`,
  `dart_output`, then `no_web` — the file is gen-recorded, so byte
  stability across regenerations matters.
- `rust_input` entries join comma-separated (`crate::api,crate::api2`).
- `dart_output` defaults to `lib/src/rust` when the spec omits it —
  always emit the key, even for the default.
- When `web = false`, emit `no_web: true` as the last key; when
  `web = true`, emit no web-related key at all (FRB's default is web-on).
- Do **NOT** add config keys the spec doesn't model (`c_output`,
  `dart_entrypoint`, …) — FRB's defaults are correct for app-shaped
  projects, and unmodeled keys make regeneration non-deterministic.

## Build sequencing

The generator is a **standalone CLI, not build_runner** — no
`dart run build_runner` step exists for FRB; sequence it explicitly:

1. **One-time integration** (first FRB setup in an app; scaffolding, not
   codegen):

   ```sh
   flutter_rust_bridge_codegen integrate --rust-crate-dir <rust_root> \
     --no-write-lib --no-integration-test
   ```

   This wires the per-platform hooks (Gradle/CocoaPods/CMake via Cargokit
   shims) so `flutter build`/`flutter run` compiles the companion crate
   natively. Surface it as a user action item; do NOT hand-author
   platform-dir build files.

   - **There is no `--integration-backend` flag.** The CLI rejects it
     (`unexpected argument`). Cargokit is the only backend; the closest
     real option is `--template app|plugin`, which defaults to `app`.
   - **`--no-write-lib --no-integration-test` are required here**, not
     optional. Without them `integrate` scaffolds its own `lib/` example
     and an integration test, overwriting spec-generated Dart.
   - `integrate` also **edits `pubspec.yaml`**, which is spec-emitted —
     see "Generated-file ownership" below for what that costs and how to
     make it survive.
   - `integrate` may scaffold its own default `flutter_rust_bridge.yaml`;
     the spec-emitted file is authoritative — after integrate runs,
     re-emit (or restore) the spec's version and diff-check before the
     next generate.
2. **Every generation pass:** `flutter_rust_bridge_codegen generate`
   (run from the app root; it reads `flutter_rust_bridge.yaml`). Re-run
   whenever the companion crate's scanned modules change — a failing or
   differently-regenerating run is the intended drift signal between the
   app and the companion crate.
3. **Web builds** (only when `web = true`):
   `flutter_rust_bridge_codegen build-web` compiles the companion crate
   to wasm before `flutter build web`.

- Do **NOT** run `generate` from inside `dart_output` or the companion
  crate — path resolution in the config is app-root-relative.

### Android: two mandatory steps

`integrate` leaves an Android project that builds green with **no Rust in
the `.apk`**. Both steps below are required; neither is optional and
neither is done for you.

1. **Swap the vendored Cargokit for the maintained fork.** FRB 2.12.0
   vendors the archived `irondash` copy, which calls Gradle's removed
   `Project.exec()`:

   ```sh
   rm -rf rust_builder/cargokit
   git clone --depth 1 https://github.com/fzyzcjy/cargokit rust_builder/cargokit
   rm -rf rust_builder/cargokit/.git
   ```

   Without it the build fails loudly: `Could not find method exec()`.

2. **Give the companion crate an underscored `[package].name`.** Cargokit
   reads `[package].name` and looks for `lib<name>.so` verbatim; cargo
   writes `lib<name_with_underscores>.so`. A hyphenated name means the two
   never meet, and Cargokit silently stages **empty** `jniLibs/` ABI
   directories. Set it in the companion crate's `spec.toml`:

   ```toml
   [project]
   name = "hello_rust"   # NOT "hello-rust"
   ```

   then `specdev-analyzer manifest --write` in that root. Declaring it in
   the spec is what makes it survive the next emission — hand-editing
   `Cargo.toml` does not.

Then **gate on the artifact, not the exit code.** After `flutter build apk
--debug`:

```sh
unzip -l build/app/outputs/flutter-apk/app-debug.apk | grep 'lib<crate_name>.so'
```

An empty result with a green build is the expected symptom of missing step
1 or 2 — both fail silently.

Also align the shim's `compileSdkVersion` with the app's — the scaffolded
`rust_builder/android/build.gradle` hardcodes an older value.

### The companion crate must declare `mod frb_generated;` itself

**Required, not optional.** The companion crate's crate root
(`src/lib.rs`) must contain:

```rust
mod frb_generated;
```

Include it in that crate's spec — an `[[items]]` entry or the module
prose, whichever the crate's profile uses — so it is present before
`generate` first runs.

**Why: `generate` injects this line at byte 0 when it is absent, and that
placement is a hard compile error for any crate whose root opens with a
module doc comment.** Rust requires inner doc comments (`//!`) to precede
every item, so an injected `mod` above them fails with:

```
error[E0753]: expected outer doc comment
 --> src/lib.rs:2:1
  | //! <the crate's module doc>
```

The generator's own injected comment concedes the point — *"This line may
not be accurate, and you can change it according to your needs."* Since a
module doc comment is standard for a spec-generated crate root, a
companion crate that does not pre-declare the `mod` breaks on the **first
generation pass**, before any Dart is written.

Declaring it yourself is what avoids this: `generate` finds the module
present and skips the injection entirely. Verified idempotent — a second
`generate` leaves the crate root byte-identical.

There is a second, independent reason, and it holds even if placement
were correct: **`src/lib.rs` is a generated, lineage-recorded file.** An
injected line lives only in the source, so the next generation pass for
that module overwrites it and the build breaks again. Anything the
companion crate needs permanently belongs in its spec, not in a line some
other tool writes into the output.

## Consuming the generated bindings

Generated Dart lands under `<dart_output>/` — `frb_generated.dart` (the
`RustLib` entrypoint) plus one file per scanned Rust module. The module
carrying the `[flutter_rust_bridge]` table wraps these for the rest of
the app; other modules depend on that module, never on the generated
files directly.

**Initialization** — the bindings are unusable until `RustLib.init()`
completes; the app's entrypoint awaits it before `runApp()`:

```dart
import 'package:my_app/src/rust/frb_generated.dart';

Future<void> initRust() async {
  await RustLib.init();
}
```

**Sync call** — a `#[frb(sync)]` Rust fn surfaces as a plain synchronous
Dart function with required named parameters:

```dart
String greetUser(String name) {
  final String message = greet(name: name);
  return message;
}
```

**Async call** — the default shape: `async fn` (or any non-sync fn)
surfaces as a `Future` and runs off the UI thread:

```dart
Future<int> checksumOf(List<int> data) async {
  final int sum = await checksum(data: data);
  return sum;
}
```

**Stream** — a Rust fn taking a `StreamSink<T>` surfaces in Dart as a
function **returning** `Stream<T>`; the sink parameter disappears from
the Dart signature:

```dart
Stream<int> counterUpTo(int limit) {
  final Stream<int> ticks = countUpTo(limit: limit);
  return ticks;
}
```

- Do **NOT** re-declare the Rust API in the spec's items or prose — the
  generated files are the single source of the binding surface; the
  wrapper module's own public API is what belongs in its items.
- Do **NOT** import generated files from modules other than the wrapping
  module — that couples the whole app to generator output layout.

## Generated-file ownership

Three parties write into an FRB-enabled app; keep their boundaries:

| Files | Owner | gen-record? |
|---|---|---|
| `flutter_rust_bridge.yaml`, wrapper-module source | this codegen | yes |
| `<dart_output>/**` (`frb_generated.*`, per-module bindings), the companion crate's `frb_generated.rs` | FRB generator | **never** |
| platform-dir hook files from `integrate` (Cargokit shims) | FRB integrate | **never** |
| the companion crate's `src/lib.rs` | **the companion crate's own spec** | yes, by that crate |

- Do **NOT** gen-record or hand-edit generator-owned files — recording
  them reports perpetual drift after every regeneration; hand-edits are
  destroyed by the next `generate` run.

- **`integrate` edits `pubspec.yaml`, and the next manifest emission
  removes the edit.** It adds `rust_lib_<project>: {path: rust_builder}`
  — the Cargokit shim package that compiles the companion crate for
  native targets. `pubspec.yaml` is emitted from the spec, so the next
  emission drops that dep and **native builds silently stop including any
  Rust**: the app still builds, and the artifact contains no `.so`.
  Declare the shim in the spec instead — `architecture.toml`
  `[[external_deps]]` with a `path`, which the pub dep vocabulary already
  supports — and it survives regeneration:

  ```toml
  [[external_deps]]
  package = "rust_lib_my_app"
  path = "rust_builder"
  ```

- **The last row is the one the boundaries get wrong.** `generate` will
  write into the companion crate's `src/lib.rs` if the `frb_generated`
  module is missing (see "The companion crate must declare
  `mod frb_generated;` itself"), and that file is owned by the companion
  crate's spec, not by any generator. Pre-declaring the module keeps the
  generator out of it. This is the one place where two generators contend
  for the same file, and the resolution is that the spec wins.

`<dart_output>/` typically nests inside the project's source root
(`lib/src/rust` under `lib/src`). That is safe: items extraction reads
only the files module specs map to, and gen-drift hashes only
gen-recorded files — neither ever scans into `<dart_output>/**`.

## Web (wasm) rules — when `web = true`

The companion crate must stay **wasm-pure**: pure-function processing
only, no platform-bound I/O. Concretely, code crossing this boundary must
not rely on `std::thread::spawn` (use FRB's `spawn!` over web workers),
tokio beyond the `rt` feature, `catch_unwind`, or 64-bit typed-data lists
(`Int64List`/`Uint64List` throw on web). Serving builds that use
threading requires COOP/COEP headers (`Cross-Origin-Opener-Policy:
same-origin`, `Cross-Origin-Embedder-Policy: require-corp`).

- Do **NOT** move transport (device I/O, sockets, permissions) into the
  companion crate to "simplify" the seam — platform-bound I/O belongs on
  the Dart side; the companion crate processes bytes it is handed.

Structure, naming, and test placement for the wrapper module follow the
project's profile's `module.conventions.md`.
