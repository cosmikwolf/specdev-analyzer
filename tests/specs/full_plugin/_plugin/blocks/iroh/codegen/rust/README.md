# iroh — Rust codegen conventions

These conventions describe how an `iroh` block table on a module spec maps
to Rust source. They layer on top of the project's profile's
`module.conventions.md`; concerns specific to peer-to-peer QUIC transport
(endpoint binding, ALPN protocol handlers, stream framing, and the channel
surface that crosses the runtime's thread boundary) live here.

Pinned to **iroh 1.0.3**. The 1.0 release renamed the core vocabulary —
`NodeId` → `EndpointId`, `NodeAddr` → `EndpointAddr`, the `discovery`
module → `address_lookup`. **Do NOT** emit any pre-1.0 name; they do not
exist in this pin, and most published examples still use them.

## Crate dependencies

This block contributes `iroh`, `tokio`, `serde` and `postcard`, plus
`iroh-mdns-address-lookup` when `address_lookup` includes `mdns`:

```toml
iroh = "1.0.3"
tokio = { version = "1", features = ["rt-multi-thread", "sync", "macros"] }
serde = { version = "1", features = ["derive"] }
postcard = { version = "1", features = ["use-std"] }
iroh-mdns-address-lookup = "0.4"   # only when address_lookup includes "mdns"
```

**Do NOT** add blob, gossip or document crates from this block — those are
separate protocols with their own ALPNs, and pulling them in here couples
every endpoint to transport it does not speak.

## Endpoint emission

The `[iroh]` scalars drive one bind call. `identity` selects the
constructor: `ephemeral` uses `Endpoint::bind`, `persisted` needs the
builder so a loaded `SecretKey` can be supplied. `relay = "n0"` maps to
`presets::N0`, `relay = "disabled"` to `presets::N0DisableRelay`.

```rust
use iroh::endpoint::{presets, BindError};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey};
use iroh_mdns_address_lookup::MdnsAddressLookup;

// identity = "ephemeral"
let endpoint: Endpoint = Endpoint::bind(presets::N0).await?;
endpoint.online().await;

// identity = "persisted", address_lookup = ["dns", "mdns"]
let endpoint: Endpoint = Endpoint::builder(presets::N0)
    .secret_key(secret)
    .address_lookup(MdnsAddressLookup::builder())
    .bind()
    .await?;
```

`presets::N0` already installs DNS/pkarr lookup, so `address_lookup =
["dns"]` needs no extra call — add `.address_lookup(...)` only for `mdns`.

For `identity = "persisted"`, emit a load-or-generate helper returning both
the key and the bytes to store. Persist `SecretKey::to_bytes()` (32 bytes)
and reload with `SecretKey::from_bytes(&bytes)`:

```rust
let secret: SecretKey = match stored {
    Some(bytes) => SecretKey::from_bytes(&bytes),
    None => SecretKey::generate(),
};
let to_store: [u8; 32] = secret.to_bytes();
```

Emit `endpoint.id()` for the `EndpointId` and `endpoint.addr()` for the
`EndpointAddr`. **Do NOT** decide where the secret key file lives — the
storage path is the project's decision; take the bytes as a parameter and
return the bytes to write.

**Do NOT** call `endpoint.online().await` on a path that must not block:
it waits for the relay assignment to complete. It belongs in start-up, not
in a request path.

## Protocol emission

`alpn` becomes one `const ALPN: &[u8]`. `role` decides which halves are
emitted: `accept` and `both` spawn a `Router`; `connect` and `both` emit
the dial path.

```rust
use iroh::endpoint::{Connection, ConnectError};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};

pub const ALPN: &[u8] = b"myorg/telemetry/0";

#[derive(Debug, Clone)]
pub struct Handler;

impl ProtocolHandler for Handler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let _peer = connection.remote_id();
        connection.closed().await;
        Ok(())
    }
}

// role = "accept" | "both"
let router: Router = Router::builder(endpoint).accept(ALPN, Handler).spawn();

// role = "connect" | "both"
let connection: Connection = endpoint.connect(addr, ALPN).await?;
```

`ProtocolHandler` requires `Send + Sync + Debug + 'static`, so the handler
struct must derive `Debug` and hold only shareable state. `accept` is
called once per inbound connection and should return only when that
connection is finished — returning early drops it.

**Do NOT** spawn a `Router` when `role = "connect"`. A Router advertises
the ALPN and accepts inbound connections; emitting one for a dial-only
endpoint opens a listener the spec did not ask for.

## Stream emission

Each `[[iroh.streams]]` entry becomes one open-or-accept path plus its
framing. `kind` selects the QUIC primitive and `direction` selects the
side: `outbound` opens, `inbound` accepts.

| kind | outbound | inbound |
|------|----------|---------|
| bidi | `conn.open_bi().await` | `conn.accept_bi().await` |
| uni  | `conn.open_uni().await` | `conn.accept_uni().await` |

`bidi` yields a `(SendStream, RecvStream)` pair; `uni` yields one half.
`carries` is used verbatim as the Rust type expression and must implement
`serde::Serialize` and `serde::Deserialize` — the type itself is declared
in the project's domain spec, never emitted by this block.

`framing = "length_prefixed"` writes a u32 little-endian byte count ahead
of each encoded message. Always emit a cap and check the decoded length
against it before allocating — the prefix arrives from the peer:

```rust
pub const MAX_FRAME: usize = 64 * 1024;

// outbound
let encoded: Vec<u8> = postcard::to_stdvec(msg)?;
let len: u32 = encoded.len() as u32;
send.write_all(&len.to_le_bytes()).await?;
send.write_all(&encoded).await?;
send.finish()?;

// inbound
let mut len_bytes = [0u8; 4];
recv.read_exact(&mut len_bytes).await?;
let len: usize = u32::from_le_bytes(len_bytes) as usize;
if len > MAX_FRAME {
    return Err("frame exceeds MAX_FRAME".into());
}
let mut body = vec![0u8; len];
recv.read_exact(&mut body).await?;
```

On a continuous `uni` stream, loop the read and treat a short read on the
length prefix as the normal end of stream rather than an error. For
`framing = "raw"`, emit `recv.read_to_end(MAX_FRAME).await?` instead and
emit exactly one message before finishing.

**Do NOT** emit `framing = "raw"` on a stream that carries more than one
message. Without a delimiter the reader cannot recover boundaries, and the
failure appears as a decode error far from its cause.

**Do NOT** multiplex several declared streams onto one QUIC stream.
Separate streams are what stops a bulk transfer from head-of-line-blocking
a latency-sensitive one, which is the reason to declare them separately.

**Do NOT** emit a stream whose `direction` contradicts its channel's
`direction`. An `outbound` stream reads from its channel, so that channel
must be `to_runtime`; an `inbound` stream writes into its channel, so that
channel must be `from_runtime`. The reverse pairing describes a stream
reading from the channel that carries data out to the caller — there is
nothing to emit. Nothing machine-checks this, so verify it before
generating: stop and report the mismatch rather than guessing which of the
two fields the author meant.

## Channel emission

Each `[[iroh.channels]]` entry becomes one bounded `tokio::sync::mpsc`
channel. `direction = "to_runtime"` carries commands in; `from_runtime`
carries events out. Emit a handle struct holding the caller's ends and a
second struct holding the runtime task's ends:

```rust
use tokio::sync::mpsc;

pub struct NetHandle {
    pub commands: mpsc::Sender<Command>,
    pub events: mpsc::Receiver<Event>,
}
```

The handle is the module's public surface. `payload_type` is used verbatim
and must be `Send`.

`overflow` selects the send path:

| overflow | emission |
|----------|----------|
| backpressure | `tx.send(msg).await` — awaits a free slot |
| drop_newest | `tx.try_send(msg)` — discard on `Full` |
| drop_oldest | producer-side ring, below |

A sender cannot evict from a tokio channel, so `drop_oldest` needs the
eviction to happen before the message is handed over: push into a
`VecDeque<Event>` capped at `capacity`, `pop_front()` when full, then flush
into the channel with `try_send` while it accepts. Count the evictions —
silent loss on a telemetry path is indistinguishable from a stalled peer.

**Do NOT** emit an unbounded channel for any entry. An unbounded channel
converts a slow consumer into unbounded memory growth, which surfaces as an
out-of-memory kill long after the fault that caused it.

**Do NOT** expose the `Endpoint`, a `Connection`, or any future from the
handle. The channels are the whole crossing surface; leaking a handle to
async machinery forces every caller to own a runtime context.

## Ticket emission

iroh 1.0.3 ships **no** ticket type — `iroh_base` exports `EndpointAddr`,
`key` and `relay_url` only. `EndpointAddr` derives `Serialize` and
`Deserialize`, so a shareable ticket is an encode plus a text encoding:

```rust
let bytes: Vec<u8> = postcard::to_stdvec(addr)?;
// then hex or base32 to taste
let addr: EndpointAddr = postcard::from_bytes(&bytes)?;
```

Trim the input before decoding — tickets arrive pasted, with whitespace.

**Do NOT** emit a ticket type from a pre-1.0 iroh crate or from
`iroh-base::ticket`. That path does not exist in this pin.

## Ownership and shutdown

The endpoint runtime owns its `Endpoint`, its `Router` and every
`Connection`, and lives on its own tokio task. Shut down in order:
`router.shutdown().await`, then `endpoint.close().await`. A `Router`
aborts when dropped, so an implicit drop cuts in-flight connections.

**Do NOT** hold a `Connection` across a channel boundary to let a caller
use it directly — that reintroduces exactly the coupling the channel
surface exists to remove.
