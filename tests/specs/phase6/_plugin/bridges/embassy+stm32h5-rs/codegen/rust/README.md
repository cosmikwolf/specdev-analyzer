# embassy+stm32h5-rs — Rust codegen conventions

These conventions describe how a `bindings."embassy+stm32h5-rs"` table
on a module spec maps to Rust source. They layer on top of the embassy
block's task/channel emission and the stm32h5-rs block's peripheral/pin
emission; this bridge handles the **handoff** of a stm32h5-rs peripheral
handle into an embassy task.

The bridge does NOT introduce a new HAL crate. The peripheral handles
are constructed via `stm32h5xx-hal` (per the stm32h5-rs block); this
bridge just wires them into spawn sites or shared statics. embassy's
async wrappers (the `embassy-stm32` crate) are out of scope for this
bridge — promote to a `v1.x` variant if a real project needs them.

## When to emit

Each `bindings."embassy+stm32h5-rs".peripheral_tasks[]` entry on a
module is one task↔peripheral binding. The entry's `task` field names
an `embassy.tasks[]` on the same module; the `peripheral` field names a
`stm32h5.peripherals[]` on the same module. The `handoff` field
(default `owned`) picks one of the two emission patterns below.

## Pattern 1 — `handoff = "owned"` (typical)

The peripheral handle is constructed in the module's init function and
moved into the task at spawn time. The task's signature takes the
handle by value; ownership is exclusive.

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

Emits:

```rust
use embassy_executor::Spawner;
use stm32h5xx_hal::pac::USART3;
use stm32h5xx_hal::serial::Serial;

#[embassy_executor::task]
async fn read_uart(uart_dev: Serial<USART3>) {
    // user code reads/writes uart_dev
    loop { /* ... */ }
}

pub fn init(spawner: Spawner, /* peripheral construction args */) {
    let uart_dev: Serial<USART3> = Serial::new(/* ... */);
    spawner.spawn(read_uart(uart_dev)).unwrap();
}
```

The peripheral type follows the stm32h5-rs block's mapping table
(`USART<n>` → `Serial<USARTn>`, `SPI<n>` → `Spi<SPIn, ...>`, etc.).
**Do NOT** wrap the `Serial<USART3>` in a `Mutex` for `owned` handoff —
the task takes it by value; sharing is not the contract.

## Pattern 2 — `handoff = "shared"` (multi-task sharing)

The peripheral handle lives in a `static` `embassy_sync::mutex::Mutex`
at module scope; tasks acquire `.lock().await` to use it. Use when
multiple embassy tasks must share the same peripheral (e.g., one task
reads, another writes the same UART).

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

Emits:

```rust
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use stm32h5xx_hal::i2c::I2c;
use stm32h5xx_hal::pac::I2C1;

static I2C_DEV: Mutex<CriticalSectionRawMutex, Option<I2c<I2C1>>> = Mutex::new(None);

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

pub fn init(spawner: embassy_executor::Spawner, /* peripheral args */) {
    let i2c_dev: I2c<I2C1> = I2c::new(/* ... */);
    {
        // critical_section::with isn't strictly needed at init (no
        // tasks running yet) but keeps the pattern uniform with
        // run-time mutators.
        let mut g = I2C_DEV.try_lock().unwrap();
        *g = Some(i2c_dev);
    }
    spawner.spawn(read_sensor()).unwrap();
    spawner.spawn(write_actuator()).unwrap();
}
```

The `Option<T>` wrapper is needed because `Mutex::new` is `const`-ctor
and most HAL handles aren't `const`-constructible. The init function
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

- **`#[interrupt]` handlers are out of scope.** A peripheral interrupt
  vector still uses `cortex-m-rt`'s `#[interrupt]` attribute on a free
  function (per the stm32h5-rs block's prose). The bridge does NOT
  emit interrupt scaffolds — those remain the project's responsibility,
  layered on top of the spawn pattern emitted here.

- **No new Cargo deps from this bridge.** All required crates come from
  the embassy block (`embassy-executor`, `embassy-sync`,
  `embassy-time`) and the stm32h5-rs block (`stm32h5xx-hal`,
  `stm32h5xx-pac`). The bridge does not contribute its own crate.
