# iroh block

Peer-to-peer QUIC transport block targeting
[iroh](https://github.com/n0-computer/iroh) 1.0.3. Contributes a namespaced
`[iroh]` table on `Module` describing **one endpoint** — its identity,
address lookup, the ALPN-identified protocol it speaks, the streams that
protocol opens, and the channels through which a caller on another thread
drives it.

## When to use this block

Inline `iroh` when your project:

- Connects peers directly by public key, with hole punching and relay
  fallback handled for you. iroh's `EndpointId` *is* an ed25519 public key,
  so identity and addressing are the same thing.
- Speaks its own wire protocol over QUIC streams — a custom ALPN, one or
  more stream kinds, its own message types.
- Needs the network to run on its own thread and be driven from elsewhere
  through a bounded channel surface, without the caller owning a runtime.

Don't use this block if:

- You want file transfer, pub/sub or synced documents rather than a custom
  protocol. Those are `iroh-blobs`, `iroh-gossip` and `iroh-docs` — separate
  crates with their own ALPNs, and they would be separate blocks. This block
  deliberately does not reach into them.
- You need plain client/server TCP or HTTP. iroh buys direct connectivity
  between peers that cannot address each other; against a public server
  endpoint that machinery is cost with no return.
- You are on a target without a tokio runtime. This block assumes std and a
  multi-threaded executor.

## Why the channel surface lives in this block (IROH-OWNS-THE-CROSSING)

The block models the endpoint runtime *including* the boundary that crosses
out of it — `[[iroh.channels]]` — rather than stopping at the `Endpoint`.
Three reasons:

1. **The boundary is transport-shaped, not consumer-shaped.** Capacity,
   overflow policy and the command/event split are decided by the traffic
   the endpoint carries — a 94 Hz telemetry stream wants `drop_oldest`, a
   connect command wants `backpressure`. None of that depends on who is
   listening.
2. **It is what makes the block usable at all.** An endpoint whose
   `Connection`s are reachable only from inside its own async context
   forces every caller to own a runtime. Declaring the crossing is what
   lets the runtime stay encapsulated.
3. **The other half is too thin to be a unit.** Whatever consumes the
   event channel drains it and reacts — a handful of lines, specific to
   that consumer. Pushing the substantial half down here and leaving that
   to be hand-authored beats inventing a bridge for it. See "Companion
   blocks" below.

## What this block contributes

Schema additions on `Module`:

```toml
[iroh]
alpn           = "myorg/telemetry/0"
role           = "both"          # accept | connect | both
identity       = "persisted"     # ephemeral | persisted
relay          = "n0"            # n0 | disabled
address_lookup = ["dns", "mdns"]

[[iroh.channels]]
name         = "commands"
direction    = "to_runtime"
payload_type = "Command"
capacity     = 32
overflow     = "backpressure"

[[iroh.channels]]
name         = "events"
direction    = "from_runtime"
payload_type = "Event"
capacity     = 4096
overflow     = "drop_oldest"

[[iroh.streams]]
name      = "signals_out"
kind      = "bidi"
direction = "outbound"
framing   = "length_prefixed"
carries   = "Signal"
channel   = "commands"     # outbound reads FROM a to_runtime channel

[[iroh.streams]]
name      = "signals_in"
kind      = "bidi"
direction = "inbound"
framing   = "length_prefixed"
carries   = "Signal"
channel   = "events"       # inbound writes INTO a from_runtime channel
```

Two entity kinds (`Stream`, `Channel`) and one intra-block relation,
`stream_on_channel`, from each stream's `channel` field — so a stream
naming a channel that was never declared is caught at validate time rather
than in the code.

### Direction coherence — author-checked, not machine-checked

An `outbound` stream *reads from* its channel and an `inbound` stream
*writes into* it. So an outbound stream must name a `to_runtime` channel
and an inbound stream a `from_runtime` one. Pairing them the other way
describes a stream reading from the channel that carries data out to the
caller, which cannot be implemented.

The `stream_on_channel` relation catches an unknown channel name but
**not** a mismatched direction. A rule pack could check the pair with
`related_attrs_allowed` over `stream_on_channel`; this block ships no
pack yet, so it is a review item, and an easy one to get wrong — note
that the two entries above differ only in `direction` and `channel`.

Note also that a symmetric peer (`role = "both"`) needs **both**
directions of every flow it participates in — two stream entries and two
channels per flow, not one of each. A single `direction = "outbound"`
stream on a `role = "both"` endpoint means that peer can send but never
receive, which is rarely what a symmetric protocol wants.

## What this block does NOT contribute

- **The message types.** `carries` and `payload_type` name types owned by
  the project's domain spec. The block declares how they travel, not what
  they are. This keeps the block from becoming a second type system.
- **Where the secret key is stored.** `identity = "persisted"` says the key
  outlives the process; the path is the project's decision.
- **Blob / gossip / document protocols.** Separate crates, separate ALPNs.
- **Anything the consumer does with a drained event.** That is outside the
  transport entirely.
- **Multiple endpoints per module, custom relay maps, QUIC datagrams,
  0-RTT, connection-migration policy.** Deferred to v1.x; promote when a
  real project needs them.

## Sharp-test rationale

- **Leg 1 — concreteness.** `iroh` names one specific crate. "Which P2P library?" cannot substitute for the name: the schema
  is shaped around iroh's own concepts (ALPN routing, `EndpointId` as a
  public key, relay-assisted hole punching), which do not transfer to
  libp2p or QUIC-generally.
- **Leg 2 — rule-pack completeness.** Every field constrains the code:
  identity, relay, ALPN and role; each stream's kind, direction, framing
  and channel; each channel's direction, capacity and overflow policy.
  The guidance carries the rules no check can verify (frame caps,
  bounded channels, shutdown order).
- **Leg 3 — no cross-block reach.** The block names no other block's
  types. The channels are declared in terms of `payload_type` and
  `capacity`, with no assumption about what drains them. The guidance
  mentions no UI toolkit, no executor beyond tokio, and no profile.

## Companion blocks

Pairs naturally with any block that owns an application's main loop, which
drains the `from_runtime` channel each iteration. That wiring is
deliberately **not** a bridge: the endpoint side is substantial and lives
here, while the consumer side is a drain-and-react loop of roughly fifteen
lines. A bridge contributing that little does not earn its files.

Note also that a bridge's endpoints resolve within one spec root, so a
layout that puts the transport in one crate and its consumer in another
cannot express such a bridge at all — another reason the crossing belongs
here.

## Deferred

- `iroh-gossip` for N-peer swarms — a separate block; the `channel` surface
  here is already shaped to carry per-peer events, so a swarm variant would
  extend rather than replace it.
- Connection quality in the spec surface. `Connection::rtt` and the direct
  vs relayed path type are readable at runtime but not declared, because
  nothing in the code is constrained by them.
- A `custom` relay option alongside `n0` / `disabled`, once a project runs
  its own relay.
