# stm32f4-rs — Rust codegen conventions

These conventions describe how a `stm32f4` block table on a module spec
maps to Rust source using `stm32f4xx-hal` and `stm32f4xx-pac`. Concerns
specific to the STM32F4 MCU family — peripheral typing, alternate-
function pin routing, interrupt handler emission — live here.

This block targets the **non-embassy** STM32F4 HAL. embassy integration
goes through the `embassy+stm32f4-rs` bridge (Phase 7+); do NOT mix
`embassy-stm32` types into emissions covered here.

## Crate dependencies

Add to `Cargo.toml`:

```toml
stm32f4xx-hal = { version = "0.22", features = ["stm32f411"] }   # or your specific MCU
cortex-m = "0.7"
cortex-m-rt = "0.7"
panic-halt = "0.2"
```

The `stm32f4xx-hal` feature flag selects the specific MCU. The project's
`spec.toml` doesn't currently encode this — it's a project-level
decision, set in Cargo.toml directly. Future enhancement: a
`stm32f4-clocks` block could pin the MCU model + clock tree and drive
the feature selection.

**Do NOT** add `embassy-stm32`, `embassy-executor`, or any other
embassy-* crate from this block. Cross-runtime peripheral wiring is the
bridge's job.

## Peripheral emission

Each `[[stm32f4.peripherals]]` entry maps to a typed handle from
`stm32f4xx-hal`. The mapping is `kind` × `instance` driven:

| kind  | HAL type pattern                                |
|-------|-------------------------------------------------|
| USART | `Serial<USART{instance}>`                       |
| SPI   | `Spi<SPI{instance}, ...>`                       |
| I2C   | `I2c<I2C{instance}>`                            |
| GPIO  | `gpio<port>::Parts` (the bank, not a pin)       |
| TIM   | `Timer<TIM{instance}>`                          |
| ADC   | `Adc<ADC{instance}>`                            |
| DMA   | `dma::StreamsTuple<DMA{instance}>`              |
| RCC   | `Rcc` (singleton; instance must be 1)           |

Peripherals are typically owned by the module struct as fields. The
init function takes the peripheral struct from `Peripherals::take()`
and constructs the typed handles.

Worked example — module `Uart` with `[[stm32f4.peripherals]] name = "uart_dev", kind = "USART", instance = 1`:

```rust
use stm32f4xx_hal::pac::USART1;
use stm32f4xx_hal::serial::Serial;

pub struct Uart {
    uart_dev: Serial<USART1>,
}

impl Uart {
    pub fn new(usart1: USART1, /* pin handles, clocks */ ...) -> Self {
        let uart_dev = Serial::new(usart1, /* ... */);
        Self { uart_dev }
    }
}
```

**Do NOT** emit `Peripherals::take()` calls inside per-module init —
the `Peripherals` singleton can only be taken once per program. The
top-level `main` (or the project's entry-point file) takes it and
distributes individual peripherals to module constructors.

## Pin emission

Each `[[stm32f4.pins]]` entry maps to a typed pin handle from the
appropriate GPIO bank. The mapping is `port` × `pin` × `mode` driven.

For `mode = "alternate"` with `alt_fn = N`:

```rust
let tx_pin = gpioa.pa9.into_alternate::<7>();
```

For `mode = "output"`:

```rust
let led = gpioa.pa5.into_push_pull_output();
```

For `mode = "input"`:

```rust
let button = gpioa.pa0.into_pull_up_input();
```

For `mode = "analog"`:

```rust
let sensor = gpioa.pa1.into_analog();
```

The GPIO bank (`gpioa`, `gpiob`, etc.) is acquired from the module's
`Peripheral` of `kind = "GPIO"`. If a module declares pins on multiple
ports without declaring the corresponding GPIO peripherals, codegen
emits a warning (the pins won't compile without their bank).

Worked example — module `Uart` with USART peripheral + tx/rx pins on
port A:

```toml
[[stm32f4.peripherals]]
name = "uart_dev"
kind = "USART"
instance = 1

[[stm32f4.peripherals]]
name = "gpio_a"
kind = "GPIO"
instance = 1     # GPIOA -> instance 1 by convention; 2 = GPIOB; etc.

[[stm32f4.pins]]
name = "tx_pin"
port = "A"
pin = 9
alt_fn = 7
mode = "alternate"

[[stm32f4.pins]]
name = "rx_pin"
port = "A"
pin = 10
alt_fn = 7
mode = "alternate"
```

Emits:

```rust
let gpioa = peripherals.GPIOA.split();
let tx_pin = gpioa.pa9.into_alternate::<7>();
let rx_pin = gpioa.pa10.into_alternate::<7>();
let uart_dev = Serial::new(peripherals.USART1, (tx_pin, rx_pin), /* ... */);
```

**Do NOT** emit pin handles before splitting the GPIO bank — calling
`.split()` consumes the bank's PAC type and produces the per-pin
fields.

## Clock config — out of scope for this block

This block does not emit clock-tree configuration. Projects using
stm32f4-rs configure clocks at top-level `main` (or via a future
`stm32f4-clocks` block). Codegen for peripherals assumes a working
clock setup is provided externally. If a project's clocks are
misconfigured, peripherals will compile fine but fail at runtime —
that's a project-integration concern, not a per-block concern.

## Interrupt handlers

Peripherals that the module wants interrupts from are wired via
`stm32f4xx_hal`'s interrupt API. The pattern uses `cortex-m-rt`'s
`#[interrupt]` attribute on a free function whose name matches the
NVIC interrupt vector:

```rust
use stm32f4xx_hal::pac::interrupt;

#[interrupt]
fn USART1() {
    // user-authored ISR body
}
```

The interrupt-vector name (`USART1` here) matches the peripheral
`kind` + `instance`. Codegen emits the ISR scaffold; the body is
left to the project author. Do **not** call non-ISR-safe operations
from interrupt handlers (allocator, blocking I/O, embassy spawn —
the last of which would also be a leg-3 violation if mentioned by
this block).

## PAC vs HAL — when to use which

`stm32f4xx-pac` exposes raw register access (`USART1.cr1.modify(...)`).
`stm32f4xx-hal` wraps the PAC in typed handles with safer construction.

- **Use HAL by default.** All peripheral and pin emissions above use
  HAL types. The HAL handles register sequencing, error checking, and
  borrow discipline.
- **Drop to PAC** for advanced configuration the HAL doesn't expose
  (e.g. low-level DMA stream priority, exotic timer modes). Codegen
  supports this via a project-level escape hatch — manual code in the
  module body that reaches into `pac::Peripherals` directly.

**Do NOT** mix PAC and HAL handles for the same peripheral instance —
once HAL has constructed a `Serial<USART1>`, the PAC `USART1` is
moved and can't be reborrowed at the PAC level. Choose one
abstraction per peripheral.
