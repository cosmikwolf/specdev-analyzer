# `mk20dx` block

Chip topology for the MK20DX128 / MK20DX256 family (Teensy 3.x): which pins a
module owns, where they sit, and what mode they are moved into.

Author-facing. This file is **not** inlined into projects.

## When to use it

Inline this when a project targets the MK20DX family and you want the pin map
in the spec rather than buried in an init function. The pin map is a design
fact — it is fixed by the board, it is what a schematic revision changes, and
it is the thing a reader most often needs and least often finds.

## What it contributes

| Table | Becomes |
|---|---|
| `[[mk20dx.pins]]` | typed pin handles, moved into the declared mode |
| `interrupt` on a pin | the port handler's trigger config and per-pin dispatch |
| `[[mk20dx.interrupts]]` | a handler per declared vector, with the right flag-clear |

**Crates: none.** See "Why it contributes nothing" below — that is a mechanical
consequence, not a design stance.

## v1 scope — pins and interrupts

Peripheral instances — UART, SPI, I2C, FTM, ADC, DMA — are deliberately out of
v1. They are not harder to model, they are harder to *document correctly*:
construction runs through per-peripheral builder configs and typed pin traits,
which is a large surface, and this block's prose currently has no compiled
witness to check it against. Adding a large body of unverifiable prose is the
failure this project has paid for before.

**Promote peripherals when the block gains a reference crate**, not before.

**Interrupts are in scope and were added deliberately** (2026-08-12), because a
project using this chip will use them. The block covers both sources: a pin's
`interrupt` trigger, which raises its **port's** vector and is dispatched by
flag, and `[[mk20dx.interrupts]]` for every other vector. The load-bearing
content is the **flag-clear obligation** — a handler that returns without
clearing re-enters forever, which is the most common way MK20 interrupt code
fails, and the clear differs per peripheral family.

Note what is deliberately *not* modelled: the vector's family is derived from
its name rather than declared, and how a handler is bound to its vector belongs
to the concurrency runtime. Neither is a field here.

## Why it contributes nothing

The HAL is not on a package registry, and it cannot be consumed by git either:
its own manifest declares the PAC by a path pointing outside its repository, so
a git fetch resolves that path to nothing. A `path` source is expressible in a
block manifest, but it renders verbatim with no rebasing — a claim about the
consuming project's filesystem that a shipped block cannot make correctly.

So the project declares the HAL itself, and this block stays topology-only.
It therefore **cannot build from its own contributions alone**, which is the
same position as any topology block whose production path runs through
something it does not own.

## Verification status — and what would fix it

**This block ships no reference crate, so its codegen prose has no compiled
witness.** That is a real weakness, and it is worth being precise about why,
because the usual reason does not apply: the API exists, is stable, and was
read from source. What is missing is a way for CI to *reach* it.

Every signature in the codegen prose was read from the HAL's own source and
cross-checked against firmware that runs on hardware. That makes it accurate
today. It does not make it self-checking.

**Any one of these would let this block gain a real gate**, in rough order of
cleanliness:

1. **Publish the PAC crates to a registry** and have the HAL depend on them by
   version. The HAL then becomes git-consumable, this block can pin a rev, and
   the reference crate compiles in CI like every other block's.
2. **Point the HAL's PAC dependency at the PAC's own git repository.** One
   commit; makes the HAL git-consumable. Costs a side-by-side checkout the
   convenience of being used directly.
3. **Check both repositories out side by side in plugin CI** and have the
   reference crate path-dep the HAL. Gates the prose without changing anything
   upstream, but bakes a directory layout into the plugin tree and leaves the
   reference unbuildable for anyone whose checkout differs.

Until one lands, treat a disagreement between the codegen prose and the HAL as
the prose being wrong.

## Pairing

Pairs with a concurrency runtime through the binding for that pairing — the
runtime supplies task structure, this block supplies the pins, and the binding
carries what neither can state alone.

## Deferred

- **Peripheral instances.** See above; gated on a reference crate.
- **Clock-tree configuration.** Project-wide rather than per-module.
- **"No two modules claim the same physical pin."** Genuinely worth checking
  and not expressible as a relation — it is a uniqueness property over the
  `(port, pin)` pair across modules, and the available graph primitives check
  uniqueness of a single field, references resolving, orphans, and cycles.
  Recorded rather than approximated: a check that catches most collisions would
  be worse than none, because it would be trusted.
