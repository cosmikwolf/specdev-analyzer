# universal-ble — implementation guidance

Rules an agent follows when it writes or edits code for a module with a `[universal_ble]` table: BLE GATT access
through the `universal_ble` package. The table declares topology; the code owns behaviour. No check verifies these
rules; review does.

## Boundaries

- The module with the table wraps BLE for the app; other modules depend on it, never on the package directly.
- One BLE package only. The package has one API on every platform: no platform branches in the code.
- Device I/O failures become the module's own error type at its boundary; raw platform exceptions do not escape.

## UUIDs and scanning

- One constant per declared service and characteristic, holding the UUID as the spec spells it, named from the spec
  `id`.
- One scan filter per module listing every declared service UUID. On the web the filter also grants access: a service
  missing from it cannot be reached after connecting, and the failure shows only at first access.
- Await the scan start (it fails when the adapter is off), and stop scanning once the device is found, before
  connecting.

## Connecting

- Connect, then discover services before any characteristic access; watch the connection stream for drops.
- Re-resolve characteristics through their declared service after every connect; never cache them across
  reconnects.

## Properties

- Each value in a characteristic's `properties` allows exactly one idiom: `read` reads once; `write` writes with a
  response; `write_without_response` writes without one; `notify` and `indicate` subscribe and listen, and
  unsubscribe when the consumer is done. Check that indications are supported when the firmware is not trusted.
- No idiom the spec does not declare, and no polling read on a notify-only characteristic.

## Platform setup

- Android permissions and the iOS and macOS usage strings live in platform directories the user owns. Give them to the
  user as action items; never write into those directories.
- Web BLE runs only in Chromium-based browsers — a product fact to tell the user.
