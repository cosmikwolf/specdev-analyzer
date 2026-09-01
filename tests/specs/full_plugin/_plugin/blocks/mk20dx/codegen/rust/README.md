# Codegen — `mk20dx` block (Rust)

How to emit source from a module's `[mk20dx]` table.

Read the project's profile's `module.conventions.md` for module-wide rules
(naming, error handling, doc comments). This file covers only what the
`[mk20dx]` table adds.

> **Verification status: compiled witness in `../../reference/rust/`.** Every
> worked example below is type-checked in CI against the published
> `mk20dx-hal` this block contributes, cross-compiled for the family's
> target triple. A mismatch between this file and the HAL fails that build
> before it can reach a downstream `/spec-gen`.

---

## 1. The crate this emits against

This block contributes `mk20dx-hal` (see `deps.yaml`), so the crate is
already in the project's manifest — emit `use mk20dx_hal::…` paths and
nothing else. Do **not** emit a manifest entry for it, and do **not** name
a PAC crate: the PAC arrives through the HAL's `mk20d7` feature, and the
bare `mk20d7` name on crates.io is an unrelated 2018 crate.

## 2. Ports are split once, then pins are moved into their modes

Pins do not exist as handles until their port is split. A port split takes
**three** things — the port register block, the matching `Pt` block, and a
reference to `Sim` (it enables the port's clock gate):

```rust
use mk20dx_hal::gpio::GpioExt;

let pins_c = dp.portc.split(dp.ptc, &dp.sim);
```

**Do NOT emit a one-argument split.** `dp.portc.split()` does not exist; the
second peripheral and the `&dp.sim` reference are both required, and omitting
the `Sim` reference would leave the port unclocked even if it compiled.

**Do NOT split the same port twice.** The split consumes the port and yields
every pin on it at once, so a module that owns pins on port C splits port C
exactly once and takes its pins from that one result.

Splitting belongs in the project's init path, once, with the resulting pins
handed to the modules that own them — not repeated per module.

## 3. Pins — `[[mk20dx.pins]]`

Each entry names a pin by the **role it plays**, and carries the chip
coordinates that locate it. The field on the port-pins struct is the port
letter lowercased followed by the pin number:

| Spec | Emitted |
|---|---|
| `port = "C"`, `pin = 5` | `pins_c.pc5` |
| `port = "A"`, `pin = 12` | `pins_a.pa12` |

The `mode` moves the pin into its typed state. One method per mode, and the
type parameters are part of the handle:

```rust
// mode = "push_pull_output"
let status_led = pins_c.pc5.into_push_pull_output();

// mode = "open_drain_output"
let bus_line = pins_b.pb0.into_open_drain_output();

// mode = "floating_input"   / "pull_up_input" / "pull_down_input"
let sense = pins_d.pd4.into_floating_input();
let button = pins_d.pd5.into_pull_up_input();
let strap = pins_d.pd6.into_pull_down_input();
```

**Alternate function takes the mux as a CONST GENERIC, not an argument:**

```rust
// mode = "alternate", alt_fn = 2
let spi_clk = pins_c.pc5.into_alternate::<2>();
```

**Do NOT emit `into_alternate(2)`.** The mux index is a type parameter — a
runtime argument does not compile. This is the single easiest thing to get
wrong in this file.

**Do NOT invent an `alt_fn` value.** Which mux index routes a pin to which
peripheral is a pin-mux table fact for the specific part. If the spec does not
give one, stop and ask; a guessed mux silently wires the pin to the wrong
peripheral, which compiles and then fails on hardware.

### Naming and typing the handle

Name the binding after the spec's `name`, not its coordinates — `status_led`,
never `pc5`. The role is what the rest of the module means; the coordinates are
an implementation fact that a board revision may change.

Where a pin is stored in a struct, annotate the full type so a wrong mode fails
at the field rather than at first use:

```rust
use mk20dx_hal::gpio::{Output, Pin, PushPull};

pub struct Display {
    reset_line: Pin<'C', 5, Output<PushPull>>,
}
```

The port is a `char` and the pin number a `u8`, both const generics — so
`Pin<'C', 5, …>`, with the port letter **quoted as a character literal and
uppercase**.

## 4. Interrupts

Two sources, and they emit differently. A pin's `interrupt` field describes a
**port** interrupt; the `[[mk20dx.interrupts]]` table declares every other
vector the module owns.

### 4.1 The one rule that applies to every handler

**A handler MUST clear its source's flag before returning.** The chip does not
clear it for you, so a handler that returns without clearing re-enters
immediately and the application appears to hang inside an ISR that looks
correct. This is the single most common way MK20 interrupt code fails.

How to clear depends on the source, and codegen derives the family from the
vector name:

| Vector | Clear by |
|---|---|
| `PORTA`…`PORTE` | writing the read `ISFR` value back (write-1-to-clear) |
| `PIT0`…`PIT3` | the timer channel's `clear_interrupt()` |
| `DMA0`…`DMA15` | the DMA channel's `clear_interrupt()` |
| anything else | its own peripheral status register — **look it up** |

**Do NOT guess a clear for a peripheral not listed above.** The HAL provides
`clear_interrupt()` on timer and DMA channels only; for other peripherals the
sequence is peripheral-specific (some clear on reading a data register, some
need an explicit write). If the spec does not make it determinable, stop and
ask rather than emitting a handler that might spin.

### 4.2 Pin interrupts — `interrupt` on a pin

**One vector serves the whole port.** Every interrupting pin on port B raises
`PORTB`, so there is exactly one handler per port and it dispatches by flag.
Emit one handler per port that has any interrupting pin, never one per pin.

Set the trigger when the pin is configured. The spec states intent; the
PCR's `IRQC` field holds the value it maps to:

| `interrupt` | `IRQC` | Meaning |
|---|---|---|
| `rising` / `falling` / `either_edge` | `1001` / `1010` / `1011` | edge-triggered |
| `level_high` / `level_low` | `1100` / `1000` | level-triggered — stays asserted while the level holds |
| `dma_rising` / `dma_falling` / `dma_either_edge` | `0001` / `0010` / `0011` | raises a **DMA request, not a CPU interrupt** |

Two facts about the write itself, both witnessed in the reference crate.
The port register block is reached through the PAC's raw pointer
(`unsafe { &*pac::Portb::PTR }`) — the port peripheral was consumed by its
split, so there is no owned handle to go through. And the PCR's `ISF` bit
is write-1-to-clear, so a `modify` that sets `IRQC` must write `ISF` to 0
explicitly (`w.irqc()._1011().isf()._0()`) — writing back the 1 it read
would clear a pending flag as a side effect of configuring the trigger.

**Do NOT emit a handler for a `dma_*` pin.** There is no interrupt to service;
emitting one produces a handler that never runs and implies wiring that does
not exist.

Dispatch reads the port's flag register, tests the bit for each pin the module
declared, and writes the flags back to clear:

```rust
// one handler for PORTB, covering every interrupting pin on that port
let isfr = /* read the port's ISFR */;
if isfr & (1 << 0) != 0 {
    // the pin declared at port = "B", pin = 0
}
if isfr & (1 << 19) != 0 {
    // the pin declared at port = "B", pin = 19
}
// write the read value back to clear exactly the flags observed
```

**Do NOT clear the whole register with a blanket write.** Write back the value
you read, so a pin that fired between the read and the write keeps its flag and
is serviced next time instead of being lost.

**Level-triggered pins need their condition removed before returning**, not
just the flag cleared — the flag re-asserts immediately while the level holds.
For a `level_high` / `level_low` pin, the handler must do whatever makes the
level go away (service the device, or disable that pin's trigger) before it
returns.

Where the project enables the HAL's `async` feature, the read-and-clear step is
available as a helper per port rather than hand-rolled — prefer it, since it
gets the write-back-what-you-read rule right. It does not exist without that
feature, so do not emit a call to it unless the project declared the feature.

### 4.3 Other vectors — `[[mk20dx.interrupts]]`

Each entry names a vector the module owns. Emit a handler for it and clear its
flag per §4.1.

**Do NOT emit the binding itself.** How a handler is attached to a vector is
the concurrency runtime's business — a task declaration in whatever runtime the
project uses, or a bare interrupt attribute if it uses none. This block says
which vector and what to clear; it does not say how the function gets there.

**Do NOT put long work in a handler.** Latch what happened and hand it to
ordinary code — an atomic, a channel, or a task the runtime wakes. A handler
that blocks delays every lower-priority interrupt on the chip.

## 5. Clock — `[mk20dx.clock]`

The tree is configured ONCE, before any peripheral exists, and every
peripheral constructor takes the frozen result:

```rust
use mk20dx_hal::clocks::{ClockSpeed, McgExt};

// speed = "mhz120"
let clocks = dp.mcg.constrain().freeze_at(ClockSpeed::Mhz120, dp.osc, &dp.sim);
```

The preset maps directly: `mhz72` / `mhz96` / `mhz120` →
`ClockSpeed::Mhz72` / `Mhz96` / `Mhz120`. All three assume the family's
standard 16 MHz crystal. `freeze_at` consumes the OSC peripheral and
borrows `Sim`.

**`mhz48` is the MK20DX128 shape and emits differently** — that part has
no `ClockSpeed` enum; emit `dp.mcg.constrain().freeze(dp.osc, &dp.sim)`.
A `mhz48` spec on an mk20d7 project (or `mhz72`+ on mk20d5) is a
part/spec mismatch: stop and ask, do not pick a nearby preset.

**AT MOST ONE module declares `[mk20dx.clock]`.** The graph primitives
cannot express this uniqueness, so it is codegen's rule: refuse to emit
when two modules declare it, naming both. Emission belongs in the
project's init path; the resulting `Clocks` value is passed by reference
to every constructor below.

**Do NOT silently downgrade an overclock.** `mhz96` / `mhz120` exceed the
72 MHz datasheet rating; that is the board's recorded design choice, and
120 MHz is the Teensy community default.

## 6. PIT timers — `[[mk20dx.timers]]`

The PIT is split ONCE, in project init, like a port:

```rust
use mk20dx_hal::timer::{PitChannel, PitExt};

let pit = dp.pit.split(&dp.sim, &clocks);
```

The split yields all four channels (`pit.ch0` .. `pit.ch3`); the module
receives the channels it declared, typed by index:

```rust
// [[mk20dx.timers]] name = "sequencer_tick", channel = 0, period_us = 600
use fugit::ExtU32;
let mut sequencer_tick: PitChannel<0> = pit.ch0;
sequencer_tick.start(600u32.micros());
```

`start` takes a `fugit` duration — `fugit` is contributed by this block's
deps for exactly this call; `ExtU32` provides the literal suffix.

**An entry without `period_us` is handed over unstarted.** The module's
own code owns its timing (a time driver, a one-shot). Do NOT invent a
period, and do NOT call `start` for it.

**The timer's interrupt is a separate declaration.** When the module also
declares the matching `PIT<n>` vector in `[[mk20dx.interrupts]]`, emit
`enable_interrupt()` after `start` (it pre-clears the pending flag) and
clear with `clear_interrupt()` in the handler per §4.1. A timers entry
alone gets polling (`wait()` / `has_expired()`), not a handler.

## 7. SPI — `[[mk20dx.spi]]`

`sck` / `mosi` / `miso` name entries in the module's own `pins` table,
already moved into `alternate` mode per §3. The constructor takes the
typed handles, and the HAL's per-instance pin traits make the compiler
settle pin-function agreement — a pin that cannot carry SCK for this
instance fails the build at the call:

```rust
use mk20dx_hal::spi::{Config, Spi, SpiExt};
use mk20dx_hal::time::U32Ext;

// name = "main_bus", instance = 0, baudrate_hz = 15000000
let main_bus: Spi<mk20dx_hal::spi::Spi0> = dp.spi0.spi(
    spi_sck, spi_mosi, spi_miso,
    Config::new(15_000_000u32.Hz()),
    &clocks, &dp.sim,
);
```

A non-zero `mode` goes through the builder: `Config::new(...).mode(MODE_2)`
(`MODE_0`..`MODE_3` re-exported from `mk20dx_hal::spi`). Omit the call
for mode 0 — it is the default.

**Name the instance marker by module path** (`mk20dx_hal::spi::Spi0`),
never by a bare import: the PAC peripheral type in scope as `dp.spi0`'s
type is ALSO called `Spi0`, and a glob or bare import silently shadows
one with the other.

**The baud is a target, not a promise.** The hardware divides the bus
clock; the driver picks the nearest rate at or below the spec's
`baudrate_hz`. Do not emit compensation logic.

## 8. UARTs — `[[mk20dx.uarts]]`

Same shape: `tx` / `rx` name `alternate`-mode pins from §3, typed pin
traits settle the function agreement:

```rust
use mk20dx_hal::uart::{Config, UartExt};
use mk20dx_hal::time::U32Ext;

// name = "midi", instance = 2, baudrate = 31250
let midi = dp.uart2.serial(
    midi_tx, midi_rx,
    Config::new(31_250u32.Hz()),
    &clocks, &dp.sim,
);
let (midi_tx_half, midi_rx_half) = midi.split();
```

Non-default framing goes through the builder:
`Config::new(...).parity(Parity::Even).word_length(WordLength::Bits9)`.
Omit both calls for the 8N1 default.

Emit the `split()` when the module's ports hand TX and RX to different
owners; keep the joined `Serial` when one owner reads and writes.

## 9. ADCs — `[[mk20dx.adcs]]`

```rust
use mk20dx_hal::adc::{Adc, AdcExt, Averaging, Resolution};

// name = "cv_inputs", instance = 1, resolution = "bits12", averaging = "x4"
let mut cv_inputs: Adc<mk20dx_hal::adc::Adc1> = dp.adc1.adc(&clocks, &dp.sim);
cv_inputs.set_resolution(Resolution::Bits12);
cv_inputs.set_averaging(Averaging::Avg4);
cv_inputs.calibrate().expect("ADC calibration failed");
```

The spec vocabulary maps `bits8`..`bits16` → `Resolution::Bits8`..`Bits16`
and `none` / `x4` / `x8` / `x16` / `x32` → `Averaging::Disabled` /
`Avg4`.. `Avg32`. Omit the setter for an absent field — the constructor's
defaults (10-bit, no averaging) stand.

**`calibrate` defaults to true and returns a `Result` — handle it.**
Calibration is what makes the stated resolution real; a swallowed failure
is an ADC quietly out of spec. How to handle it is the project's error
policy; `expect` is the floor, not the recommendation. Emit no
`calibrate()` call only when the spec says `calibrate = false`.

Which input channel a `read` samples is a call-site fact in the module's
own code, not part of construction.

## 10. DMA channels — `[[mk20dx.dma_channels]]`

The controller is split ONCE, in project init:

```rust
use mk20dx_hal::dma::{DmaChannel, DmaExt, DmaSource};

let dma = dp.dma.split(dp.dmamux, &dp.sim);
```

The module receives its declared channels, typed by index, and each entry
with a `source` is wired at init:

```rust
// name = "display_flush", channel = 1, source = "SPI0_TX"
let mut display_flush: DmaChannel<1> = dma.ch1;
display_flush.set_source(DmaSource::SPI0_TX);
```

The `source` string is the HAL constant's name, verbatim
(`DmaSource::SPI0_TX`). **Do NOT invent one** — whether the name exists
is settled by the compiler, and the DMAMUX request table is a chip fact
this block does not enumerate. An entry without `source` is a
software-triggered channel: hand it over unwired.

A channel that raises a completion interrupt is, again, a separate
`DMA<n>` entry in `[[mk20dx.interrupts]]`, cleared per §4.1.

## 11. FTM timers — `[[mk20dx.ftms]]`

An entry WITH `frequency_hz` is running PWM: emit the one-call
constructor, then configure each declared channel:

```rust
use mk20dx_hal::pwm::FtmExt;
use mk20dx_hal::time::U32Ext;

// name = "gate_pwm", instance = 0, frequency_hz = 20000
let gate_channels = dp.ftm0.pwm(20_000u32.Hz(), &clocks, &dp.sim);

// channels: name = "gate_1", channel = 2
let mut gate_1 = gate_channels.ch2;
gate_1.set_pwm();
gate_1.set_value(0);
```

`pwm()` starts the counter but every channel starts disabled — the
`set_pwm()` per declared channel is what produces output, and the initial
duty is 0 unless the spec's module prose says otherwise.

**Duty on a single channel is `set_value`, not `set_duty`.** `set_value`
writes the CnV compare against the timer's modulo (the embedded-hal
`SetDutyCycle` impl goes through the same call); `set_duty` exists only
on the combine-mode channel PAIR, which this block does not model —
emitting it on a channel does not compile.

An entry WITHOUT `frequency_hz` is manual-configuration ownership
(combine mode, input capture): emit `dp.ftm0.split(&clocks, &dp.sim)`
and hand the parts over stopped — the module's own code takes it from
there. Do NOT start the counter for it.

**Which package pin a channel drives is §3's business.** Declare the pin
in `pins` with `mode = "alternate"` and the part's ALT index (FTM pins
are typically ALT3 or ALT4 — a pin-mux table fact; do not guess). The
HAL does not take pins in the FTM constructor, so nothing here connects
them — the schema deliberately mirrors that.

## 12. What this block does not emit

**Do NOT emit the runtime binding for a handler.** Section 4 covers what a
handler must do; how it gets attached to its vector belongs to whichever
runtime the project uses.

**Do NOT emit I2C, I2S, USB, or the small analog blocks** (CMP / VREF /
DAC). They are out of the block's modelled surface; a module needing one
gets it from the project's own init code until the block covers it, with
its witness added in the same commit.
