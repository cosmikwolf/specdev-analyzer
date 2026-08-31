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

**Crates: `mk20dx-hal`**, from the registry, with the `mk20d7`, `rt` and
`critical-section` features (`codegen/rust/deps.yaml`). The PAC arrives
through the `mk20d7` feature — never name a PAC crate directly; the bare
`mk20d7` name on crates.io is an unrelated 2018 crate.

## v1 scope — pins and interrupts

Peripheral instances — UART, SPI, I2C, FTM, ADC, DMA — are deliberately out of
v1. They are not harder to model, they are harder to *document correctly*:
construction runs through per-peripheral builder configs and typed pin traits,
which is a large surface. That was gated on the block having a compiled
witness for its prose, and the gate is now met — the block ships a
reference crate. Peripherals and the clock tree are the next authoring
pass, not a deferral.

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

## Verification status

**The codegen prose has a compiled witness: `reference/rust/`.** It
type-checks every emission kind the block models — pins in every mode the
schema admits, a pin-trigger port interrupt with its dispatch, and a
`[[mk20dx.interrupts]]` vector with its flag clear — against the published
`mk20dx-hal` the block contributes, cross-compiled for the family's
target triple, with `unused_crate_dependencies` denied so a contributed
dep nothing exercises fails the build. CI compiles it like every other
block's reference crate.

The block was zero-crate until 2026-08-31: the HAL's own manifest used to
declare its PAC by a path outside its repository, which made it
consumable neither from a registry nor from git. That was resolved
upstream by publishing `mk20d7-pac` / `mk20d5-pac` and `mk20dx-hal` to
crates.io with by-version PAC deps.

## Pairing

Pairs with a concurrency runtime through the binding for that pairing — the
runtime supplies task structure, this block supplies the pins, and the binding
carries what neither can state alone.

## Deferred

- **Peripheral instances.** See above; the reference-crate gate is met,
  so these are next-pass work rather than deferred.
- **Clock-tree configuration.** Project-wide rather than per-module; same
  next pass.
- **"No two modules claim the same physical pin."** Genuinely worth checking
  and not expressible as a relation — it is a uniqueness property over the
  `(port, pin)` pair across modules, and the available graph primitives check
  uniqueness of a single field, references resolving, orphans, and cycles.
  Recorded rather than approximated: a check that catches most collisions would
  be worse than none, because it would be trusted.
