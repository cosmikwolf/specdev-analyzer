# iroh — implementation guidance

Rules an agent follows when it writes or edits code for a module with an `[iroh]` table: a peer-to-peer QUIC
endpoint, its protocols, streams, and the channels that cross into the rest of the program. No check verifies them;
review does.

## Scope

- iroh renamed its core vocabulary at its first stable release, and most published examples use the old names. Check every
  name against the version in `Cargo.toml`.
- No blob, gossip or document crates from this block: those are other protocols with their own ALPNs.

## Endpoint

- `identity` picks the bind path: `ephemeral` generates a key; `persisted` loads a stored secret key or generates one,
  and returns the bytes to store. Where the key is stored is the project's choice: take the bytes in, give the bytes
  out.
- `relay` picks the relay preset; add an address-lookup service only for what the preset does not already include.
- Waiting for the endpoint to be online blocks until relay assignment: start-up only, never a request path.

## Protocols

- `alpn` is one constant. `role = "accept"` or `"both"` runs a router that accepts the ALPN; `role = "connect"` or
  `"both"` dials. A dial-only endpoint runs no router — that would open a listener the spec did not ask for.
- A protocol handler holds only shareable state, and returns from its accept method only when that connection is
  finished.

## Streams

- Each `[[iroh.streams]]` entry is one stream: `kind` picks bidirectional or unidirectional, `direction` picks
  opening (`outbound`) or accepting (`inbound`). Never multiplex several declared streams onto one QUIC stream.
- `carries` is the project's serializable type, never declared by this block.
- `length_prefixed`: a little-endian length before each message, checked against a maximum frame size before any
  allocation (the length comes from the peer). On a continuous stream, a short read of the length is the normal end.
- `raw` carries exactly one message, read to the end under the same cap.
- An `outbound` stream reads from a `to_runtime` channel; an `inbound` stream writes into a `from_runtime` channel. If
  the spec pairs them the other way, stop and report it.

## Channels

- Each `[[iroh.channels]]` entry is one bounded channel of `payload_type` (which must be `Send`); `to_runtime` carries
  commands in, `from_runtime` carries events out. Never an unbounded channel.
- `overflow`: `backpressure` waits for a slot; `drop_newest` discards on full; `drop_oldest` evicts on the producer
  side before handing over. Count every drop.
- The handle of channel ends is the module's whole public surface: never expose the endpoint, a connection or a
  future through it, or hand a connection across a channel.

## Tickets and shutdown

- A shareable ticket is the endpoint address serialized and text-encoded; trim pasted input before decoding.
- The runtime task owns the endpoint, router and connections. Shut the router down, then close the endpoint; a
  dropped router aborts in-flight connections.
