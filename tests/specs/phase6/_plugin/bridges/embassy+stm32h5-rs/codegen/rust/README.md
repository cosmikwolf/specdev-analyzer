# embassy+stm32h5-rs — Rust codegen conventions

These conventions describe how a `bindings."embassy+stm32h5-rs"` table
on a module spec maps to Rust source. They layer on top of the embassy
block's task/channel emission and the stm32h5-rs block's peripheral/pin
topology; this bridge handles the **handoff** of an STM32H5 peripheral
handle into an embassy task.

The bridge owns the **join-product crate**: `embassy-stm32`, the
embassy-flavored STM32 HAL. It exists only because these two blocks are
paired — embassy alone is HAL-agnostic, stm32h5-rs alone is
runtime-agnostic — so the dependency and the type vocabulary below
belong to the bridge, not to either block. When this bridge is inlined,
its `embassy-stm32` type mappings replace the stm32h5-rs block's
(forward-looking) `stm32h5xx-hal` patterns for every task-owned
peripheral: bridge prose wins over block prose for cross-block wiring.

## Crate dependencies

This bridge contributes one crate (see `deps.yaml`):

```toml
embassy-stm32 = { version = "0.6", features = ["stm32h563zi", "rt", "time-driver-any", "exti"] }
```

- The chip feature is a **full part number** (`stm32h563zi`, the
  Nucleo-H563ZI verification target) — embassy-stm32 requires exactly
  one. Note this is NOT the `[platform].mcu_part` family prefix
  (`stm32h563`); projects on a different H5 part override by declaring
  `embassy-stm32` under `[[external_deps]]` in `architecture.toml` with
  their part's feature — user-authored deps win the merge.
- `time-driver-any` wires embassy-time's driver to an H5 timer; do not
  also enable a specific `time-driver-tim*` feature.
- `memory-x` is deliberately NOT enabled — the profile owns `memory.x`
  emission and a crate-emitted copy would conflict with a hand-authored
  one.

## Entry-point integration

`embassy_stm32::init` replaces both the PAC `Peripherals::take()`
pattern and any clock-setup scaffolding in the entry point:

Each typed handle is **constructed at the entry point** (where the
`Peripherals` and `Irqs` live) and passed by value into the owning
module's `init`. Raw `Peri` fields never cross the module boundary —
only finished handles do:

```rust
#[embassy_executor::main]
async fn main(spawner: embassy_executor::Spawner) {
    let p = embassy_stm32::init(embassy_stm32::Config::default());

    // GPIO output: construct then hand off.
    let led = embassy_stm32::gpio::Output::new(
        p.PB0,
        embassy_stm32::gpio::Level::Low,
        embassy_stm32::gpio::Speed::Low,
    );
    blinky::init(&spawner, led);

    // Async USART with GPDMA: construct then hand off.
    let uart = embassy_stm32::usart::Uart::new(
        p.USART3, p.PD9, p.PD8,        // peri, rx, tx
        p.GPDMA1_CH0, p.GPDMA1_CH1,    // tx_dma, rx_dma
        Irqs,                          // interrupt bindings
        embassy_stm32::usart::Config::default(),
    )
    .unwrap();
    uart_echo::init(&spawner, uart);
}
```

`Uart::new`'s argument order is `(peri, rx, tx, tx_dma, rx_dma, irq,
config)` and it returns `Result<Uart, ConfigError>` — unwrap at the
entry point. `#[embassy_executor::main]` (from the embassy block's
executor) is the entry attribute; do NOT also emit
`#[cortex_m_rt::entry]` — the macro expands to it. `embassy_stm32::init`
configures clocks (default config runs from HSI; project-specific clock
trees go in a `Config` value documented in `architecture.toml` notes)
and returns the one-time `Peripherals` struct. **Do NOT** call
`embassy_stm32::init` anywhere but the entry point.

Async peripherals need their interrupt handlers bound once at crate
root via `bind_interrupts!`. A DMA-driven USART binds BOTH the USART
interrupt AND each GPDMA channel interrupt (channel `N` → vector
`GPDMA1_CHANNEL<N>`):

```rust
use embassy_stm32::{bind_interrupts, dma, peripherals, usart};

bind_interrupts!(struct Irqs {
    USART3 => usart::InterruptHandler<peripherals::USART3>;
    GPDMA1_CHANNEL0 => dma::InterruptHandler<peripherals::GPDMA1_CH0>;
    GPDMA1_CHANNEL1 => dma::InterruptHandler<peripherals::GPDMA1_CH1>;
});
```

## When to emit

Each `bindings."embassy+stm32h5-rs".peripheral_tasks[]` entry on a
module is one task↔peripheral binding. The entry's `task` field names
an `embassy.tasks[]` on the same module; the `peripheral` field names a
`stm32h5.peripherals[]` on the same module. The `handoff` field
(default `owned`) picks one of the two emission patterns below.

The peripheral type follows embassy-stm32's instance-erased handles:

| stm32h5 `kind` | embassy-stm32 type (task-side)                     |
|----------------|-----------------------------------------------------|
| USART / UART   | `usart::Uart<'static, Async>`                       |
| SPI            | `spi::Spi<'static, Async>`                          |
| I2C            | `i2c::I2c<'static, Async>`                          |
| GPIO (pin out) | `gpio::Output<'static>`                             |
| GPIO (pin in)  | `gpio::Input<'static>` / `exti::ExtiInput<'static>` |
| TIM            | `timer::simple_pwm::SimplePwm<'static, T>`          |
| ADC            | `adc::Adc<'static, T>`                              |

`stm32h5.pins[]` entries on a bound module map to `gpio::Output` /
`gpio::Input` handles constructed from the pin's `port`+`pin` peripheral
(e.g. `PB0`) at the entry point and passed into the module's init.

## Pattern 1 — `handoff = "owned"` (typical)

The peripheral handle is constructed at the entry point (or in the
module's init from raw peripherals) and moved into the task at spawn
time. The task's signature takes the handle by value; ownership is
exclusive.

Worked example — module `Uart` with one embassy task `read_uart` and
one stm32h5-rs peripheral `uart_dev`:

```toml
name = "Uart"
layer = "driver"

[[stm32h5.peripherals]]
name = "uart_dev"
kind = "USART"
instance = 3

[[embassy.tasks]]
name = "read_uart"
pool_size = 1

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "read_uart"
peripheral = "uart_dev"
handoff = "owned"
```

Emits (module file):

```rust
use embassy_stm32::mode::Async;
use embassy_stm32::usart::Uart;

#[embassy_executor::task]
async fn read_uart(mut uart_dev: Uart<'static, Async>) {
    // user code reads/writes uart_dev
    loop { /* ... */ }
}

pub fn init(spawner: &embassy_executor::Spawner, uart_dev: Uart<'static, Async>) {
    spawner.spawn(read_uart(uart_dev).unwrap());
}
```

The `Uart` is constructed at the entry point (where `Peripherals` and
`Irqs` live) and passed in. **Do NOT** wrap an `owned` handle in a
`Mutex` — the task takes it by value; sharing is not the contract.

## Pattern 2 — `handoff = "shared"` (multi-task sharing)

The peripheral handle lives in a `static` `embassy_sync::mutex::Mutex`
at module scope; tasks acquire `.lock().await` to use it. Use when
multiple embassy tasks must share the same peripheral (e.g., one task
reads, another writes the same I2C bus).

Worked example — module `Bus` with two tasks sharing one I2C peripheral:

```toml
name = "Bus"
layer = "driver"

[[stm32h5.peripherals]]
name = "i2c_dev"
kind = "I2C"
instance = 1

[[embassy.tasks]]
name = "read_sensor"
pool_size = 1

[[embassy.tasks]]
name = "write_actuator"
pool_size = 1

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "read_sensor"
peripheral = "i2c_dev"
handoff = "shared"

[[bindings."embassy+stm32h5-rs".peripheral_tasks]]
task = "write_actuator"
peripheral = "i2c_dev"
handoff = "shared"
```

Emits (module file):

```rust
use embassy_stm32::i2c::I2c;
use embassy_stm32::mode::Async;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;

static I2C_DEV: Mutex<CriticalSectionRawMutex, Option<I2c<'static, Async>>> =
    Mutex::new(None);

#[embassy_executor::task]
async fn read_sensor() {
    let mut g = I2C_DEV.lock().await;
    let i2c = g.as_mut().unwrap();
    // user code uses i2c
}

#[embassy_executor::task]
async fn write_actuator() {
    let mut g = I2C_DEV.lock().await;
    let i2c = g.as_mut().unwrap();
    // user code uses i2c
}

pub fn init(spawner: &embassy_executor::Spawner, i2c_dev: I2c<'static, Async>) {
    {
        let mut g = I2C_DEV.try_lock().unwrap();
        *g = Some(i2c_dev);
    }
    spawner.spawn(read_sensor().unwrap());
    spawner.spawn(write_actuator().unwrap());
}
```

The `Option<T>` wrapper is needed because `Mutex::new` is `const`-ctor
and HAL handles aren't `const`-constructible. The init function
populates the `Some(_)` once. Tasks `unwrap()` because by the time they
run, init has already filled the slot — programming error to spawn
before `init`.

**Do NOT** use `core::cell::RefCell` here — embassy's `Send` rules
forbid holding a non-`Send` borrow across `.await`. embassy_sync's
`Mutex` is `Send`-safe.

## Cross-cutting rules

- **No cross-module bindings in v1.** Both endpoints (`task` and
  `peripheral`) must be declared on the same module. The schema's
  identifier pattern doesn't enforce this — it's a convention the
  reduction's `endpoints.from`/`endpoints.to` templates depend on
  (`${parent.name}.${.peripheral}` resolves the peripheral within the
  module). Cross-module wiring (sharing a peripheral across modules)
  is not in scope; if needed, model it as a higher-level
  `architecture.toml` connection or a future bridge variant.

- **Interrupt binding lives at the entry point.** Async peripherals
  require `bind_interrupts!` (see "Entry-point integration"); the
  bridge emits that binding once at crate root, NOT per module. Raw
  `#[interrupt]` free-function handlers (the stm32h5-rs block's
  non-embassy pattern) must not coexist with a `bind_interrupts!` entry
  for the same vector.

- **This bridge's deps come from its `deps.yaml`.** The embassy block
  contributes the runtime crates (`embassy-executor`, `embassy-sync`,
  `embassy-time`); the stm32h5-rs block currently contributes none; the
  bridge contributes `embassy-stm32`. Do not hand-mirror any of these
  in `[[external_deps]]` unless overriding a version or chip feature.
