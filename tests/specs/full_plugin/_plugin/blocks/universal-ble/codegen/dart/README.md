# universal-ble — Dart codegen guidance

How a `[universal_ble]` table on a module becomes working code. The
table declares GATT topology (services, characteristics, consumed
properties); codegen owns everything behavioral — UUID constants, the
scan filter, the connect flow, and one access idiom per declared
characteristic property. The module carrying the table wraps BLE for
the rest of the app; other modules depend on that module, never on
`package:universal_ble` directly.

## Package dependency

The block contributes `universal_ble` (see `deps.yaml`). It is a
federated Flutter plugin with one uniform API across android / ios /
macos / windows / linux / web — emitted code never branches on
platform.

- Do **NOT** add `flutter_blue_plus` (or any second BLE package)
  alongside it: on web, flutter_blue_plus silently returns "sensible
  defaults" instead of erroring, which masks real failures.

## UUID constants and the scan filter

For each declared service and characteristic, emit one `const String`
holding the UUID **verbatim as spelled in the spec** (the package
accepts short or long form, any case, and normalizes internally —
re-spelling it at emission time only creates diff noise). Names derive
from the spec `id` by snake_case → lowerCamelCase plus a
`ServiceUuid` / `CharacteristicUuid` suffix.

Emit exactly one scan-filter function per module, listing **every**
declared service UUID in declaration order. Its name is fixed
(`gattScanFilter`) — it lives in the module's own file, so two BLE
modules never collide; a consumer importing both files uses an import
prefix:

```toml
[[universal_ble.services]]
id = "battery"
uuid = "180F"

[[universal_ble.characteristics]]
id = "battery_level"
service = "battery"
uuid = "2A19"
properties = ["read", "notify"]
```

```dart
const String batteryServiceUuid = '180F';
const String batteryLevelCharacteristicUuid = '2A19';

ScanFilter gattScanFilter() {
  return ScanFilter(withServices: [batteryServiceUuid]);
}

Future<Stream<BleDevice>> scanForPeripherals() async {
  await UniversalBle.startScan(scanFilter: gattScanFilter());
  return UniversalBle.scanStream;
}

Future<void> stopScanning() async {
  await UniversalBle.stopScan();
}
```

Await `startScan` (it fails loudly when the adapter is off or the
platform refuses), and stop the scan once the target device is found —
before connecting, not after.

- Do **NOT** scan without the filter or with a partial service list:
  on web, `withServices` doubles as Web Bluetooth's
  `optional_services` — a service missing from the scan filter is
  **inaccessible after connecting**, and the failure appears only at
  the first read/write, far from its cause.

## Connecting and service discovery

Connect on the `BleDevice` a scan yielded, then discover before any
characteristic access. Monitor the device's `connectionStream`
(`Stream<bool>`; `false` = dropped) for disconnects:

```dart
Future<List<BleService>> connectAndDiscover(BleDevice device) async {
  await device.connect();
  final List<BleService> services = await device.discoverServices();
  return services;
}

StreamSubscription<bool> watchConnection(
    BleDevice device, void Function() onDrop) {
  return device.connectionStream.listen((bool connected) {
    if (!connected) {
      onDrop();
    }
  });
}
```

- Do **NOT** cache `BleCharacteristic` objects across reconnects —
  re-resolve via `getCharacteristic` after each connect;
  characteristic handles from a previous session are stale.

## Per-property access idioms

Each value in a characteristic's `properties` selects exactly one
emission. Resolve the characteristic through its declared service —
`getCharacteristic` takes the characteristic UUID positionally and the
service UUID as the required named `service` parameter. A helper may
resolve internally from a `BleDevice` (read example below) or accept a
characteristic resolved earlier in the **same connect session** (notify
example) — the binding rule is re-resolution after every reconnect, not
a particular signature:

**read** — one-shot value read:

```dart
Future<Uint8List> readBatteryLevel(BleDevice device) async {
  final BleCharacteristic characteristic = await device.getCharacteristic(
      batteryLevelCharacteristicUuid,
      service: batteryServiceUuid);
  final Uint8List value = await characteristic.read();
  return value;
}
```

**write** — acknowledged write (`withResponse: true`):

```dart
await characteristic.write(value, withResponse: true);
```

**write_without_response** — fire-and-forget:

```dart
await characteristic.write(value, withResponse: false);
```

**notify** — subscribe, then listen on `onValueReceived`; unsubscribe
when the consumer is done:

```dart
Future<Stream<Uint8List>> subscribeBatteryLevel(
    BleCharacteristic characteristic) async {
  await characteristic.notifications.subscribe();
  return characteristic.onValueReceived;
}

Future<void> stopBatteryLevel(BleCharacteristic characteristic) async {
  await characteristic.notifications.unsubscribe();
}
```

**indicate** — identical shape via `indications`; indication values
arrive on the same `onValueReceived` stream. Check `isSupported` first
when the peripheral's firmware is not fully trusted:

```dart
if (characteristic.indications.isSupported) {
  await characteristic.indications.subscribe();
}
```

- Do **NOT** emit an idiom the spec doesn't declare (a write helper
  for a read-only characteristic compiles fine and fails at runtime on
  the device), and do **NOT** call `read()` on a notify-only
  characteristic to "poll" — subscribe instead.
- Write payloads are `List<int>`; received values are `Uint8List`
  (import `dart:typed_data`).

## Error handling

Operations throw typed exceptions (`ConnectionException`,
`PairingException`, and `UniversalBleException` as the base — all
exported by `package:universal_ble/universal_ble.dart`). Wrap
device I/O at the module boundary and surface failures through the
module's own error type per the project's profile's
`module.conventions.md` — do **NOT** let raw platform exceptions
escape the wrapping module.

## Platform permission manifests (user action items — never emitted)

BLE needs platform-manifest entries that live in externally-owned
platform directories. Surface these as **user action items** with the
exact snippets; do **NOT** write into platform dirs and do **NOT**
gen-record any file there.

Android (`android/app/src/main/AndroidManifest.xml`):

```xml
<uses-permission android:name="android.permission.BLUETOOTH_CONNECT" />
<uses-permission android:name="android.permission.BLUETOOTH" android:maxSdkVersion="30" />
<uses-permission android:name="android.permission.BLUETOOTH_ADMIN" android:maxSdkVersion="30" />
<uses-permission android:name="android.permission.ACCESS_COARSE_LOCATION" android:maxSdkVersion="28" />
<uses-permission android:name="android.permission.ACCESS_FINE_LOCATION" android:maxSdkVersion="30" />
<uses-permission android:name="android.permission.BLUETOOTH_SCAN" android:usesPermissionFlags="neverForLocation" />
```

(If the app derives location from BLE — iBeacons — remove the
`maxSdkVersion` caps on the location permissions.)

iOS/macOS (`Info.plist`): `NSBluetoothAlwaysUsageDescription` and
`NSBluetoothPeripheralUsageDescription`, each with a user-facing
reason string; macOS additionally needs the Bluetooth capability in
Xcode. Web needs no permission setup (the browser prompts per
connection).

## Web

The scan-filter rule above is the load-bearing part. Note for the app
author (product-level, not emitted): browser BLE support is limited to
Chromium-based browsers — BLE features of a web build will not run in
Safari or Firefox regardless of what the spec declares.

Structure, naming, and test placement for the wrapping module follow
the project's profile's `module.conventions.md`.
