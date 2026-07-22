# flutter-app: Dart module conventions

These conventions describe how a `flutter-app` profile module spec maps
to Dart/Flutter source. They are language-and-architecture-only —
framework specifics (FFI bridges, BLE stacks, state-management
libraries) come from the relevant block's `codegen/dart/README.md`,
layered on top, as Dart blocks ship in the plugin tree.

## File layout

Each module emits as `lib/src/<snake>.dart` — `UserSettings` →
`lib/src/user_settings.dart`. One module, one file; helpers private to
the module live in the same file with `_`-prefixed names.

**The `app`-role module additionally owns `lib/main.dart`**, a thin
delegation stub:

```dart
import 'package:<package_name>/src/app.dart';

void main() => runApp(const App());
```

All composition logic (MaterialApp, theming, routing, DI bootstrap)
lives in the app module's own file, not in `main.dart` — the stub only
delegates. Both files are gen-owned and both are recorded at emission.

Do **not** create `lib/<package_name>.dart` barrel exports for an app —
apps have no external consumers; barrels only blur the module graph.

## Role semantics

`module.role` states the module's architectural tier and drives the
`role_dependency` ordering check (upward dependencies warn):

| role               | contains                                              | may depend on (per the check) |
|--------------------|-------------------------------------------------------|---------------|
| `app`              | composition root: MaterialApp/shell, route table, DI bootstrap | anything |
| `screen`           | one routable page (a `Scaffold`-rooted widget)        | screen, widget, service, platform_channel, model |
| `widget`           | reusable UI below screen level                        | widget, service, platform_channel, model |
| `service`          | non-UI logic and state: notifiers, repositories, controllers | service, platform_channel, model |
| `platform_channel` | the Dart side of a platform channel contract          | model |
| `model`            | pure Dart data types and domain logic                 | model |

The table states what the `role_dependency` check permits: same-role and
downward edges. Advisory on top of that (not checked): route UI-tier
platform access through a service rather than importing a
platform_channel module from a screen or widget directly — the check's
total order cannot flag that skip-level, but the service detour is the
intended structure.

Emission guidance per role:

- **`app`** — owns the single `runApp` entry (via `main.dart` above),
  the route table, and theme construction. Nothing depends on `app`; a
  screen that needs navigation uses the navigation API (route names,
  context-based navigation), never an import of the route table.
- **`screen`** — one top-level widget per screen module, named after the
  module (`SettingsScreen`). Prefer `StatelessWidget` + externalized
  state in a service; reach for `StatefulWidget` only for genuinely
  local, ephemeral state (text-field controllers, animation
  controllers).
- **`widget`** — stateless where possible; take data and callbacks via
  constructor parameters rather than reaching into services directly
  when the widget is meant to be reusable.
- **`service`** — expose state as `ChangeNotifier` / `ValueNotifier` /
  stream-shaped APIs per the project's decisions. **Co-locate providers
  and notifiers with their service**, not in the `app` module — an
  app-level provider barrel would force every screen into an upward
  `screen → app` dependency.
- **`platform_channel`** — see "Platform channels" below.
- **`model`** — no Flutter imports at all (`dart:` and pure-Dart
  packages only). Immutable where practical: `final` fields,
  `const` constructors, `copyWith` for updates, sealed class hierarchies
  for closed variants.

## Platform matrix

`architecture.toml`'s `platforms` array is the app's target matrix. A
module omitting `platforms` supports **all** of it; a module declaring
`platforms = [...]` is fenced to that subset. Codegen consumes the
field in two ways:

1. **Doc header.** Every emitted module file opens with a doc comment
   stating its platform contract:

   ```dart
   /// BLE device scanning and connection state.
   ///
   /// Platforms: android, ios (fenced — not available on web).
   library;
   ```

   For unfenced modules: `/// Platforms: all targets.`

2. **Web-excluded modules get a guard.** When a fenced module excludes
   `web` but the project matrix includes it, dependents that also run
   on web must not import it unconditionally. Emit the conditional
   guard at the *consumer's* usage site using `kIsWeb`:

   ```dart
   import 'package:flutter/foundation.dart' show kIsWeb;

   if (kIsWeb) {
     // capability notice / fallback path
   } else {
     // fenced-module path
   }
   ```

   When the fenced code cannot even be *compiled* for the excluded
   platform (platform-specific imports), use conditional exports in a
   facade file instead:

   ```dart
   export 'device_scan_unsupported.dart'
       if (dart.library.io) 'device_scan_io.dart';
   ```

   Prefer the `kIsWeb` runtime guard when both branches compile
   everywhere; reach for conditional export only when they don't.

A dependency from a wider-matrix module onto a narrower-fenced one is
legal in the spec — the guard idiom above is how the generated code
honors the fence. State the fence in prose in the module description
when it is load-bearing.

## Platform channels

A `platform_channel`-role module's `channels[]` array is the
cross-language contract: the native side must register exactly these
names. Emission:

- One `const` name per channel, then the channel object built from it:

  ```dart
  /// Platform channel names owned by this module. The native side
  /// registers these exact strings.
  const String batteryMethodChannelName = 'com.example.app/battery';
  const String batteryEventChannelName = 'com.example.app/battery_events';

  const MethodChannel _batteryChannel = MethodChannel(batteryMethodChannelName);
  const EventChannel _batteryEvents = EventChannel(batteryEventChannelName);
  ```

- `kind = "method"` → a `MethodChannel` plus one Dart method per
  platform call, converting `PlatformException` into the module's typed
  failures.
- `kind = "event"` → an `EventChannel` exposed as a typed
  `Stream<T>` via `receiveBroadcastStream().map(...)`.
- `kind = "basic_message"` → a `BasicMessageChannel` with an explicit
  codec; rare — prefer method/event channels unless the contract is
  genuinely message-shaped.

The channel *names* come from the spec verbatim — never invent or
rename one at emission. The native-side handler implementations live in
the platform directories, which are externally owned (see "Generated-
file ownership") — codegen emits the Dart side only and the module's
doc header lists what the native side must implement.

## Item emission

`module.items[]` carries the module's contractual surface. The rule is
one line:

> An item is a **public, contractual code symbol, identified by its
> qualified name, described in prose — never by signature.**

Dart specifics:

- A method is a function: `Device.connect` is a `kind = "function"`
  item the same way a top-level `connectAll` is. Members qualify with
  `.` — `<Declaration>.<member>`.
- Privacy is lexical. `_`-prefixed names (or public members of a
  `_`-prefixed declaration) are never items.
- Constructors are not items — the class item's description covers
  construction. Fields are not items — expose contractual state via
  getters when it is contract, or fold it into the class description.
- `@override` members are not items: an override implements an
  inherited contract (a widget's `build`, a state's `createState`,
  `toString`), and the contract lives on the declaring base. Do not
  author them; extraction skips them symmetrically.
- The shape is **prose-first**: each item carries a `description` of
  what it does, NOT a signature mirror. Never name parameters, return
  types, or type arguments — the spec describes the *contract*, the
  source describes the *implementation*. `signature_hint` is a rare
  escape hatch (FFI-adjacent declarations); avoid it for ordinary
  members.

Example — a screen module's surface:

```toml
[[items]]
name = "SettingsScreen"
kind = "class"
description = "Routable settings page: theme toggle and account row."
satisfies = ["REQ-UI-003"]

[[items]]
name = "SettingsScreen.routeName"
kind = "getter"
description = "Stable route identifier used by the app's route table."
satisfies = ["REQ-UI-003"]
```

A public declaration left out of `items[]` still compiles but shows as
`in_source_only` in `/spec-sync-items` — a correct signal that you have
public surface the spec doesn't account for. Reconcile by authoring the
member (with `satisfies`) or prefixing it `_`.

Sparseness rule (enforced by the language schema): every item carries
`satisfies` (≥1 requirement id) unless it is a test. Many members may
share one coarse requirement — reach for requirement granularity, not
signatures, when the load feels heavy.

## Test items

An `[[items]]` entry with `test = true` is a test: `kind = "function"`,
`verifies = [...]` (pointing at `nature = "test"` requirements) in place
of `satisfies`, and a `test_kind` driving emit location:

| `test_kind`   | Emit location                                                    |
|---------------|------------------------------------------------------------------|
| `unit`        | A `test()` case in `test/<module>_test.dart`.                    |
| `widget`      | A `testWidgets()` case (flutter_test) in `test/<module>_test.dart`. |
| `integration` | An entry under `integration_test/`, driven on a device/emulator.  |
| `golden`      | A `testWidgets()` case asserting `matchesGoldenFile(...)` in `test/<module>_test.dart`; the golden image itself is generated by the test runner (`--update-goldens`), not by codegen. |

Body emission follows the standard discipline: write a working test when
the verification context (requirement text + `verified_by` hints +
decision `verification` criteria) carries enough to do so. When it
genuinely doesn't, emit a failing marker rather than a vacuous pass:

```dart
test('golden render', () {
  fail('verifies REQ-TEST-602: criteria not yet specified');
});
```

Test-only packages (`mocktail`, `golden_toolkit`) are declared as
`external_deps` entries with `test_only = true`; the manifest step
routes them to `dev_dependencies`. `flutter_test` itself is declared
once, at architecture level:

```toml
[[external_deps]]
package = "flutter_test"
sdk = "flutter"
test_only = true
```

## Cross-module dependencies

`module.depends_on[]` does not directly emit code — imports appear
naturally as the body is generated. When a module uses a sibling, emit
a package import:

```dart
import 'package:<package_name>/src/<sibling_snake>.dart';
```

Use package imports (not relative `../` imports) for cross-module
references inside `lib/` — uniform paths keep tooling and moves clean.
Do not import a module the spec doesn't list in `depends_on` — add the
edge first.

## External pub dependencies

`module.external_deps[]` declares pub packages the module needs. Each
entry is a PubReference — `package` required; exactly one source key
(`hosted`/`git`/`path`/`sdk`) or none for pub.dev; registry deps
require `version`; `test_only = true` routes to `dev_dependencies`.
The union across modules, architecture, and inlined blocks/bridges is
what the manifest step emits — pubspec.yaml is OUTPUT, never
hand-maintained.

```toml
[[external_deps]]
package = "collection"
version = "^1.19.0"
```

The Flutter SDK dependency itself (`{ package = "flutter", sdk =
"flutter" }`) is declared once at architecture level, not per module.

## Dart style

- Null safety idioms: no `dynamic` in public surface; prefer `final`
  everywhere a binding isn't reassigned; `late` only when
  initialization order genuinely requires it.
- `const` constructors and `const` widget instantiation wherever the
  analyzer allows — it is both idiom and a rebuild-cost win.
- Errors: throw typed exception classes (one per failure family, named
  `<Domain>Exception`); never throw strings. `platform_channel` modules
  convert `PlatformException` at the boundary.
- Async: return `Future`/`Stream` directly; no fire-and-forget
  `unawaited` calls without a stated reason.
- Follow Effective Dart naming: types `UpperCamelCase`, members and
  files `lowerCamelCase`/`snake_case.dart`.

## Refusal rule

Emitting `throw UnimplementedError()` (or `UnimplementedError` in any
dress) as a function body is **refused output** — the same rule as every
other language's stub markers. If the spec lacks the information to
implement a member, stop and report which module + item is
underspecified instead of emitting a stub. The single exception is the
test-item failing marker shown above, which uses `fail(...)` — a test
that *runs and fails loudly* — precisely so unimplemented verification
is visible, not silent.

## Generated-file ownership

Three generators may touch a Flutter tree; only one of them is
spec-owned:

| Generator            | Owns                                                        | gen-recorded? |
|----------------------|-------------------------------------------------------------|---------------|
| `flutter create` / SDK tooling | platform dirs (`android/`, `ios/`, `web/`, `macos/`, `windows/`, `linux/`), `analysis_options.yaml`, `.metadata` | **never** |
| third-party codegen (FFI bridges, build_runner outputs) | their declared outputs (`*.g.dart`, `*.freezed.dart`, bridge glue) | **never** |
| `/spec-gen`          | `lib/main.dart`, `lib/src/*.dart`, `test/`, `integration_test/`, `pubspec.yaml` | always |

Recording an externally-owned file would report perpetual `edited`
drift the moment its own generator reruns. Codegen must not create,
modify, or record files in the externally-owned set — platform-side
channel handlers, launcher icons, and manifest permissions are authored
in the platform dirs by hand (or by their own tooling) and referenced
from spec prose only.
