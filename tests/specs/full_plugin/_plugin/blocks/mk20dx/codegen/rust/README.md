# Codegen — `mk20dx` block (Rust)

How to emit source from a module's `[mk20dx]` table.

Read the project's profile's `module.conventions.md` for module-wide rules
(naming, error handling, doc comments). This file covers only what the
`[mk20dx]` table adds.

> **Verification status: this prose has no compiled witness.** Every signature
> below was read from the HAL's own source and cross-checked against working
> firmware, but nothing recompiles it when the HAL changes. Treat a mismatch
> between this file and the HAL as this file being wrong, and fix it here.

---

## 1. The crate this emits against is declared by the project

This block contributes no dependency. The HAL arrives from the project's own
`[[external_deps]]`, because it is consumed by a local path that only the
project knows. Emit `use mk20dx_hal::…` paths normally; do **not** emit a
manifest entry for it, and do not assume a crate name other than the one the
project declared.

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

## 4. What this block does not emit

**Do NOT emit peripheral construction here** — UART, SPI, I2C, timers, ADC,
DMA. This block's v1 models pins only. A module needing a peripheral gets it
from the project's own init code until the block covers them.

**Do NOT emit clock configuration.** The clock tree is project-wide, not a
per-module concern, and is set up once before any module sees a pin.

**Do NOT emit interrupt handlers here.** How a pin's edge becomes a task is a
concurrency concern belonging to whichever runtime the project uses; that
binding lives outside this block.
