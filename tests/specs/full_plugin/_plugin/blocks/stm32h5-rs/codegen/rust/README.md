# stm32h5-rs — Rust codegen conventions

These conventions describe how an `stm32h5` block table on a module
spec maps to Rust source using `stm32h5xx-hal` and `stm32h5xx-pac`.
Concerns specific to the STM32H5 MCU family — peripheral typing,
alternate-function pin routing, GPDMA, ICACHE, USB-OTG-FS — live here.

This block targets the **non-embassy** STM32H5 HAL. Embassy integration
goes through the `embassy+stm32h5-rs` bridge; do NOT mix `embassy-stm32`
types into emissions covered here.

Default verification target: STM32H563 + Nucleo-H563ZI dev board.
Variants (H503, H523, H573) follow the same shape; peripheral counts
differ.

## Crate dependencies

This block contributes **no crates today** (`deps.yaml` is empty). The
non-embassy STM32H5 HAL this prose targets (`stm32h5xx-hal`) has no
usable crates.io release yet — only a 0.0.0 placeholder. The emission
patterns in this document are forward-looking against that crate's API
and activate when it ships:

```toml
# Future, once stm32h5xx-hal publishes a usable release:
stm32h5xx-hal = { version = "<released>", features = ["<mcu_part>"] }
stm32h5xx-pac = "<released>"
```

Until then, the production path for STM32H5 peripherals is
`embassy-stm32` — owned by the `embassy+stm32h5-rs` **bridge**, never
by this block. When that bridge is inlined, its type mappings replace
this document's `stm32h5xx-hal` patterns for every task-owned
peripheral (bridge prose wins over block prose for cross-block wiring).
A module using this block WITHOUT the embassy bridge currently has no
buildable peripheral-handle emission; the spec topology (peripherals,
pins, interrupts) still validates and drives codegen structure.

The `stm32h5xx-hal` feature flag (once released) selects the specific
MCU family member. **Spec coupling:** read `[platform].mcu_part` from
`architecture.toml` and use it as the HAL feature flag (lowercase, no
punctuation: `stm32h563`, `stm32h573`, `stm32h503`, etc.). The
`mcu_part` field is the single source of truth for which H5 variant
the project targets.

**Codegen-time soft check:** if the project sets `[platform].mcu_part`
but no `stm32h5-rs` block is inlined, this block can't run — but the
profile codegen prose emits a warning comment in `main.rs`:
`// SPEC WARNING: [platform].mcu_part = "<value>" but no MCU block found`.
Conversely, if this block is inlined but `mcu_part` is unset, emit
`// SPEC WARNING: stm32h5-rs block inlined but [platform].mcu_part is unset`
and fall back to a sensible default (typically `stm32h563`). Both
warnings surface as in-source comments — not analyzer findings — per
the sharp-test discipline (manual review, not enforcement).

`cortex-m`, `cortex-m-rt`, panic handlers, and logger crates do **not**
come from this block — they're declared at the project level via
`[platform]` / `[runtime]` / `[logger]` in `architecture.toml`.

**Do NOT** add `embassy-stm32`, `embassy-executor`, or any other
embassy-* crate from this block. Cross-runtime peripheral wiring is the
bridge's job.

## Peripheral emission

Each `[[stm32h5.peripherals]]` entry maps to a typed handle from
`stm32h5xx-hal`. The mapping is `kind` × `instance` driven:

| kind     | HAL type pattern                                                    |
|----------|---------------------------------------------------------------------|
| USART    | `Serial<USART{instance}>`                                           |
| UART     | `Serial<UART{instance}>` (UART4/5/7/8/9/12)                         |
| SPI      | `Spi<SPI{instance}, ...>`                                           |
| I2C      | `I2c<I2C{instance}>`                                                |
| GPIO     | `gpio<port>::Parts` (the bank — instance: 1=A, 2=B, ..., 8=H)       |
| TIM      | `Timer<TIM{instance}>` (general-purpose) or `pwm::Pwm<...>` (advanced) |
| ADC      | `Adc<ADC{instance}>`                                                |
| DMA      | `dma::GpDma<GPDMA{instance}>` (NOT stream-tuple — H5 uses GPDMA channels) |
| RCC      | `Rcc` (singleton; instance must be 1)                               |
| USB      | `usb_otg::USB1` (USB-OTG-FS; H563 single instance)                  |
| OCTOSPI  | `octospi::Octospi<OCTOSPI{instance}>`                               |

Peripherals are typically owned by the module struct as fields. The
init function takes the peripheral struct from `Peripherals::take()`
and constructs the typed handles.

Worked example — module `Uart` with `[[stm32h5.peripherals]] name = "uart_dev", kind = "USART", instance = 3`:

```rust
use stm32h5xx_hal::pac::USART3;
use stm32h5xx_hal::serial::Serial;

pub struct Uart {
    uart_dev: Serial<USART3>,
}

impl Uart {
    pub fn new(usart3: USART3, /* pin handles, clocks */ ...) -> Self {
        let uart_dev = Serial::new(usart3, /* ... */);
        Self { uart_dev }
    }
}
```

**Do NOT** emit `Peripherals::take()` calls inside per-module init —
the `Peripherals` singleton can only be taken once per program. The
top-level `main` (or the project's entry-point file) takes it and
distributes individual peripherals to module constructors.

## Pin emission

Each `[[stm32h5.pins]]` entry maps to a typed pin handle from the
appropriate GPIO bank. The mapping is `port` × `pin` × `mode` driven.

For `mode = "alternate"` with `alt_fn = N`:

```rust
let tx_pin = gpiod.pd8.into_alternate::<7>();
```

For `mode = "output"`:

```rust
let led = gpiob.pb0.into_push_pull_output();
```

For `mode = "input"`:

```rust
let button = gpioc.pc13.into_pull_up_input();
```

For `mode = "analog"`:

```rust
let sensor = gpioa.pa1.into_analog();
```

The GPIO bank (`gpioa`, `gpiob`, etc.) is acquired from the module's
`Peripheral` of `kind = "GPIO"`. Standard convention: `instance = 1`
maps to GPIOA, `2` to GPIOB, ..., `8` to GPIOH. If a module declares
pins on multiple ports without declaring the corresponding GPIO
peripherals, codegen emits a warning (the pins won't compile without
their bank).

Worked example — module `LedDriver` with three onboard Nucleo-H563ZI
LEDs:

```toml
[[stm32h5.peripherals]]
name = "gpio_b"
kind = "GPIO"
instance = 2

[[stm32h5.peripherals]]
name = "gpio_f"
kind = "GPIO"
instance = 6

[[stm32h5.peripherals]]
name = "gpio_g"
kind = "GPIO"
instance = 7

[[stm32h5.pins]]
name = "led_green"
port = "B"
pin = 0
mode = "output"

[[stm32h5.pins]]
name = "led_yellow"
port = "F"
pin = 4
mode = "output"

[[stm32h5.pins]]
name = "led_red"
port = "G"
pin = 4
mode = "output"
```

Emits:

```rust
let gpiob = peripherals.GPIOB.split();
let gpiof = peripherals.GPIOF.split();
let gpiog = peripherals.GPIOG.split();
let led_green = gpiob.pb0.into_push_pull_output();
let led_yellow = gpiof.pf4.into_push_pull_output();
let led_red = gpiog.pg4.into_push_pull_output();
```

**Do NOT** emit pin handles before splitting the GPIO bank — calling
`.split()` consumes the bank's PAC type and produces the per-pin
fields.

## Clock config — out of scope for this block

This block does not emit clock-tree configuration. STM32H5 has a
sophisticated clock tree (HSE up to 50 MHz, PLL1/PLL2/PLL3, separate
peripheral clock domains) configured at top-level `main` or via a
future `stm32h5-clocks` block. Codegen for peripherals assumes a
working clock setup is provided externally.

For Nucleo-H563ZI quick-start: HSE_BYPASS at 8 MHz from STLink MCO,
SYSCLK target 250 MHz via PLL1, peripheral clocks divided as needed.
Document the project's specific config in `architecture.toml` notes.

## GPDMA — different from F4's stream model

STM32H5 uses **GPDMA** (General-Purpose DMA), which is request-channel-
oriented rather than F4's stream/channel pair. Each GPDMA controller
(GPDMA1, GPDMA2) has 8 channels; each channel carries a request line
mapped to a specific peripheral (e.g. USART3_RX → GPDMA1 channel N).

Emission: declare the GPDMA peripheral via `kind = "DMA"` with the
appropriate instance; the HAL's `dma::GpDma<GPDMA1>` handle is acquired
in init. Channel-to-peripheral wiring is set at peripheral construction
time per the HAL's API. Specifics depend on which peripheral; consult
the HAL docs.

## USB-OTG-FS

STM32H563 has one USB-OTG-FS instance. Emission via stm32h5xx-hal's
`usb_otg::USB1` requires:

- Pin assignments: PA11 (D-) and PA12 (D+) on GPIOA, mode = "alternate",
  alt_fn = 10.
- USB peripheral: `kind = "USB"`, `instance = 1`.
- VBUS sensing: optional, project-specific.

The HAL's USB support is currently in active development; some endpoint
classes may require dropping to PAC-level access via stm32h5xx-pac. See
"PAC vs HAL" below.

## ICACHE setup

STM32H5 has an instruction cache (ICACHE peripheral) that should be
enabled at boot for full performance. The HAL's ICACHE support is
typically configured at top-level `main` — not per-module. Document
the recommendation in the project's architecture; per-module emission
does not configure ICACHE.

## Interrupt handlers

Peripherals that the module wants interrupts from are wired via
`stm32h5xx-hal`'s interrupt API. The pattern uses `cortex-m-rt`'s
`#[interrupt]` attribute on a free function whose name matches the
NVIC interrupt vector:

```rust
use stm32h5xx_hal::pac::interrupt;

#[interrupt]
fn USART3() {
    // user-authored ISR body
}
```

The interrupt-vector name (`USART3` here) matches the peripheral
`kind` + `instance`. Codegen emits the ISR scaffold; the body is left
to the project author. Do **not** call non-ISR-safe operations from
interrupt handlers (allocator, blocking I/O, embassy spawn — the last
of which would also be a leg-3 violation if mentioned by this block).

## PAC vs HAL — when to use which

`stm32h5xx-pac` exposes raw register access (`USART3.cr1.modify(...)`).
`stm32h5xx-hal` wraps the PAC in typed handles with safer construction.
The HAL is currently maturing — coverage is solid for USART/SPI/I2C/
GPIO/TIM/ADC but thinner for newer peripherals (USB device classes,
crypto accelerators, OCTOSPI advanced modes).

- **Use HAL by default.** All peripheral and pin emissions above use
  HAL types. The HAL handles register sequencing, error checking, and
  borrow discipline.
- **Drop to PAC** for advanced configuration the HAL doesn't yet expose
  (e.g. TrustZone-aware register access, OCTOSPI memory-mapped mode,
  advanced timer features). Codegen supports this via a project-level
  escape hatch — manual code in the module body that reaches into
  `pac::Peripherals` directly.
- **Use stm32-metapac** as an alternative PAC if `stm32h5xx-pac` is
  ahead-of-version on a feature you need. Document the choice at
  project level; do NOT mix metapac with stm32h5xx-pac for the same
  peripheral instance.

**Do NOT** mix PAC and HAL handles for the same peripheral instance —
once HAL has constructed a `Serial<USART3>`, the PAC `USART3` is moved
and can't be reborrowed at the PAC level. Choose one abstraction per
peripheral.
