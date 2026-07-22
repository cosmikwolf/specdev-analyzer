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
   codegen): `flutter_rust_bridge_codegen integrate
   --integration-backend cargokit --rust-crate-dir <rust_root>` wires the
   per-platform hooks (Gradle/CocoaPods/CMake via Cargokit shims) so
   `flutter build`/`flutter run` compiles the companion crate natively.
   Cargokit is the required backend when `web = true` — the alternative
   build-hooks backend is native-only. Surface this as a user action
   item; do NOT hand-author platform-dir build files. `integrate` may
   scaffold its own default `flutter_rust_bridge.yaml`; the spec-emitted
   file is authoritative — after integrate runs, re-emit (or restore)
   the spec's version and diff-check before the next generate.
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

- Do **NOT** gen-record or hand-edit generator-owned files — recording
  them reports perpetual drift after every regeneration; hand-edits are
  destroyed by the next `generate` run.

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
