# `universal-ble` block

BLE central-role access for Flutter apps via the
[universal_ble](https://pub.dev/packages/universal_ble) package
(Navideck): one uniform Dart API across android / ios / macos /
windows / linux / web. A module declares the GATT topology it consumes
— services and characteristics with the properties the app relies on —
and the guidance holds the code's scan filter, connect flow, and
per-property access idioms to it.

## When to use

The app talks to a BLE peripheral: scans for it, connects, and reads /
writes / subscribes to GATT characteristics. Inline this block into
the app-root project and put the `[universal_ble]` table on the module
that owns the BLE seam (typically a service-role module the rest of
the app depends on). An app talking to two peripheral kinds can carry
the table on two modules.

## What this block contributes

- `[universal_ble]` schema table: `services[]` (id + uuid) and
  `characteristics[]` (id + parent service + uuid + consumed
  properties from `read` / `write` / `write_without_response` /
  `notify` / `indicate`).
- Reduction entities `Service` and `Characteristic` plus the
  `characteristic_in_service` relation — a typo'd service reference is
  a validate-time error, not a runtime surprise on real hardware.
- Implementation guidance covering UUID constants, the scan filter
  (every declared service, which the web target requires),
  connect/discovery, one access idiom per property, error-handling
  posture, and the android/ios permission entries as user action items.
- The project's `pubspec.yaml` depends on `universal_ble`.

## What this block does NOT contribute

- **Peripheral role** (advertising / GATT server): universal_ble
  supports it, this block doesn't model it. Add on a real driver.
- **Pairing/bonding surface, descriptors, MTU tuning**: available in
  the package API, consumed ad hoc in hand-authored code if needed;
  not spec vocabulary until a project shows the need.
- **Scan filters beyond service UUIDs** (manufacturer data, name
  prefix): app logic for now.
- **The three unconsumed GATT properties** (`broadcast`,
  `authenticated_signed_writes`, `extended_properties`): the package
  exposes no API that reads them, so a spec value could not constrain
  the code (the dead-flag rule). `broadcast`'s real
  use-case — beacon-style consumption of advertised data without
  connecting — is scan-level in this package's API, and would enter
  the schema as an advertisement-data surface, not a characteristic
  property. All are additive if a trigger fires.

## Web caveat (product-level)

Web Bluetooth is **Chromium-only** (Chrome / Edge / Opera; no Safari,
no Firefox, no iOS browsers). BLE features of a web build simply will
not run elsewhere — no spec mechanism changes that; plan the product
accordingly. Additionally, on web the scan filter's service list is
the access boundary: services not declared in the spec (and therefore
not in the scan filter) cannot be reached after connecting.

## Why universal_ble

- `flutter_blue_plus`: no honest web path — unsupported features
  return "sensible defaults" instead of errors on web, masking real
  failures.
- `btleplug` (Rust-side BLE): no wasm/web support (still an unmerged
  PR upstream) — putting BLE in a companion Rust crate would forfeit
  the web target and break wasm-purity. BLE belongs on the Dart side.

## Sharp-test rationale

- **Concreteness:** names the specific pub.dev package `universal_ble`,
  not "a BLE block".
- **Rule-pack completeness:** each service, characteristic and property
  constrains the code (the scan filter, the allowed access idioms), and
  `characteristic_in_service` resolves at validate time; the guidance
  carries the rest.
- **No cross-block reach:** stands alone. No other block's types
  appear in the guidance.

## Companion blocks

Pairs naturally with the `flutter-rust-bridge` block in the same app
root (BLE transport on the Dart side, pure processing in the companion
Rust crate) — with **no coupling between the two blocks**: bytes flow
device → this block's streams → app code → FFI calls. No bridge
exists or is needed; if a project ever wants spec-level wiring between
BLE characteristics and Rust functions, that is a new bridge, not an
edit to either block.
