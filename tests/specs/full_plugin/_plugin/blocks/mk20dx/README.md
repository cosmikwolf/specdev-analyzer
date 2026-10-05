# `mk20dx` block

Chip topology for the MK20DX128 / MK20DX256 family (Teensy 3.x): which pins,
interrupt vectors, and peripheral instances a module owns, and the
construction-time facts the board fixes for each.

Author-facing. This file is **not** inlined into projects.

## When to use it

Inline this when a project targets the MK20DX family and you want the pin map
in the spec rather than buried in an init function. The pin map is a design
fact — it is fixed by the board, it is what a schematic revision changes, and
it is the thing a reader most often needs and least often finds.

## What it contributes

| Table | States |
|---|---|
| `[[mk20dx.pins]]` | which pins the module owns, and each pin's mode |
| `interrupt` on a pin | the pin's trigger, raised on its port's vector |
| `[[mk20dx.interrupts]]` | every other vector the module handles |
| `[mk20dx.clock]` | the one-per-project clock-tree preset (`freeze_at`) |
| `[[mk20dx.timers]]` | the PIT channels the module owns, and their periods |
| `[[mk20dx.spi]]` | the SPI masters the module owns, on its own alternate pins |
| `[[mk20dx.uarts]]` | the UARTs the module owns, on its own pins |
| `[[mk20dx.adcs]]` | the ADCs the module owns, and their configuration |
| `[[mk20dx.dma_channels]]` | the DMA channels the module owns, and their DMAMUX sources |
| `[[mk20dx.ftms]]` | the FTM timers the module owns: PWM channels, or the parts for manual use |

The guidance says what the code builds from each.

**Crates the project's `Cargo.toml` needs: `mk20dx-hal`** (with the `mk20d7`,
`rt` and `critical-section` features) **and `fugit`** for the timer-period
literals. The PAC arrives through the `mk20d7` feature — never name a PAC crate
directly; the bare `mk20d7` name on crates.io is an unrelated 2018 crate.

## v2 scope — pins, interrupts, clock, and six peripheral surfaces

v2 adds the clock preset and the peripheral instances (PIT / SPI / UART /
ADC / DMA / FTM) that v1 deferred. Peripheral entries model OWNERSHIP and construction-time facts;
runtime use of a handle is the module's code. Still out: I2C, I2S, USB,
and the small analog blocks (CMP / VREF / DAC) — promote each when a
project needs it, with its guidance checked against the HAL in the same
commit.

SPI and UART entries name their pins from the module's own `pins` table,
and those references are declared as reduction relations — a name that
resolves to nothing is a validation error with source-line provenance
(`spi_uses_sck_pin` and friends), proven by a red-path control at
authoring time.

Where a field is legal on only one part of the family (SPI1, ADC1, FTM2,
DMA channels 4-15 are MK20DX256-only), the schema admits the union and
the compiler settles it — the handle's field does not exist on the
smaller part. Same philosophy as interrupt vector names.

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

The guidance (`guidance.md`) carries the rules for the code. They were checked by compiling a test crate against the
published `mk20dx-hal`, one module per kind, while the block was written. That corrected the old prose once:
single-channel PWM duty is `set_value` (CnV), while `set_duty` exists only on the combine-mode channel pair.

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

- **I2C, I2S, USB, CMP / VREF / DAC.** Out of the modelled surface until a
  project needs one; each promotion checks its guidance against the HAL
  in the same commit.
- **FTM combine mode and input capture.** An `ftms` entry without
  `frequency_hz` hands the split parts over for the module's own code —
  the manual surface (channel pairs, deadtime, quadrature) is used, not
  modelled.
- **The same PIT channel, DMA channel or peripheral instance in two
  modules.** The same shape as the pin check in `checks.toml`
  (`mk20dx.pins_unique`, a composite-key `unique` over `port` and `pin`),
  not yet added for these tables. `[mk20dx.clock]`'s at-most-one-module is a
  guidance rule.
