# firmware-layered: Rust module conventions

These conventions describe how a `firmware-layered` profile module spec
maps to Rust source. They are language-and-architecture-only — framework
specifics (embassy, RTIC, stm32-hal, lvgl-rs) come from the relevant
block's `codegen/rust/README.md`, layered on top.

## File layout

Emit each module as `src/<layer>/<name>.rs`. The layer subdirectory makes
file paths self-documenting (a reader sees `src/hal/uart.rs` and knows
its layer without opening the file). For a project with one module per
layer, use `src/<layer>.rs` files instead of `src/<layer>/`.

`src/lib.rs` (or `src/main.rs` for an embedded-bin) declares each
populated `<layer>` as a `mod` and re-exports public items per the
project's idiom. Do **not** emit a `mod.rs` unless multiple modules
share the same parent layer.

```text
src/
  lib.rs              # mod hal; mod drivers; mod application;
  hal/
    uart.rs
    spi.rs
  drivers/
    sensor.rs
  application/
    main.rs
```

## Type emission

Each module's name becomes a public struct (PascalCase), declared at the
top of the module file. Fields are derived from `metadata.MemoryModel`
and the block's contributions:

- `metadata.MemoryModel.allocation = "static_alloc"` → no `Box<…>`,
  state lives in the struct directly. Implementations use `&'static mut
  Self` or `Mutex<…>` patterns the relevant block prescribes.
- `metadata.MemoryModel.allocation = "heap_alloc"` → fields holding
  owned heap data use `alloc::boxed::Box`, `alloc::vec::Vec`, etc.
  (with `extern crate alloc;` at crate root).

`module.errors[]` becomes a `pub enum <Name>Error` with one variant per
entry, derived `#[derive(Debug, Clone, Copy, PartialEq, Eq)]` for
non-payload variants.

Worked example — module `Uart` with `errors = [{variant: BufferOverrun},
{variant: ParityError}]`:

```rust
pub struct Uart { /* fields from hardware/metadata */ }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UartError {
    BufferOverrun,
    ParityError,
}
```

Do **not** derive `Default` unless `metadata.MemoryModel.allocation` is
`static_alloc` AND the struct has no required-init fields. Do **not**
emit `derive(Serialize, Deserialize)` — it bloats no_std builds and is a
project-level decision.

## Action emission

Each `actions[]` entry becomes a method on the module struct. The
emission rules:

- `params[]` are by-value for primitives, `&` for borrowed slices/refs,
  `&mut` for mutable refs, owned for `direction = "out"` returns.
- `returns` is the Rust return type. If `metadata.ErrorHandling.strategy
  = "result_type"`, wrap the return in `Result<_, <Name>Error>`. If
  `strategy = "option"`, wrap in `Option<_>`. Other strategies follow
  the rules in the **Error handling** section below.
- `blocking = true` → method signature is `&mut self` and returns
  synchronously. `blocking = false` → method must be async (`async fn`)
  unless a block (e.g. embassy) prescribes a different idiom.
- `isr_safe = true` → emit `#[inline]` and add `// SAFETY: callable
  from interrupt context` comment. Method must not call any blocking
  primitive (a per-block lint enforces this at audit time).

Worked example — action `read_byte(timeout_us: i32) -> u8` on `Uart`
with `result_type` errors:

```rust
impl Uart {
    pub fn read_byte(&mut self, timeout_us: i32) -> Result<u8, UartError> { /* ... */ }
}
```

Do **not** emit a free function for an action; actions always belong to
their module struct's `impl` block.

## State-machine emission

Each `state_machines[]` entry becomes a sibling type to the module
struct, named `<MachineName>State` (a fieldless enum). The module struct
holds one `<MachineName>State` field per state machine. Transitions
become a `step` method that consumes an `Event` and returns the new
state.

Worked example — state machine `LedState` with states `[Off, On]` and
transitions `[{from: Off, to: On, event: power_on}, {from: On, to: Off,
event: power_off}]`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedState { Off, On }

#[derive(Debug, Clone, Copy)]
pub enum LedEvent { PowerOn, PowerOff }

impl LedState {
    pub fn step(self, event: LedEvent) -> Self {
        match (self, event) {
            (LedState::Off, LedEvent::PowerOn) => LedState::On,
            (LedState::On, LedEvent::PowerOff) => LedState::Off,
            _ => self,
        }
    }
}
```

Do **not** emit transitions that the spec didn't list — `step` returns
`self` for unspecified `(state, event)` pairs (as above).

## Error handling

`metadata.ErrorHandling.strategy` drives the action signature:

| Strategy        | Return wrapping                            |
|-----------------|--------------------------------------------|
| `result_type`   | `Result<T, <Name>Error>`                   |
| `option`        | `Option<T>`                                |
| `enum_variant`  | `T` where `T` is `<Name>Result` (custom)   |
| `panic`         | bare `T`; failures call `panic!`           |
| `errno`         | `i32` return; document non-zero meanings   |

Default to `result_type` when no strategy is declared. `panic` is
acceptable for HAL-layer init paths but not for higher layers — a
warning-level finding flags this in audit.

Do **not** mix strategies within one module's actions.

## Hardware bindings

`hardware.pins[]` entries become associated constants on the module
struct. `hardware.peripherals[]` entries become typed fields whose type
is provided by the relevant block (e.g. `embassy_stm32::peripherals::I2C1`).
`hardware.clocks[]` are documented in the module's doc comment but emit
no code unless a block prescribes otherwise.

Worked example — `hardware.pins = [{name: tx_pin, port: A, pin: 9}]`:

```rust
impl Uart {
    pub const TX_PIN_PORT: char = 'A';
    pub const TX_PIN_NUMBER: u8 = 9;
}
```

Do **not** hard-code MCU-family-specific types (e.g. `stm32f4xx_hal::gpio::Pin`)
in the emission — that decision lives in the relevant block.

## no_std and alloc attributes

For `[project].language = "rust"`, the project's `lib.rs` (or `main.rs`)
gets:

```rust
#![no_std]
#![cfg_attr(test, allow(unused))]
```

Add `extern crate alloc;` at crate root **only if** any module has
`metadata.MemoryModel.allocation = "heap_alloc"`. Never add `extern crate
std;` — projects targeting hosted environments are out of scope for
firmware-layered (use `library-api` instead).

Do **not** emit `#![allow(dead_code)]` blanket suppressions — let the
project author add suppressions per-warning if needed.

## Cross-module references

`depends_on[]` becomes either a struct field (when the module owns an
instance of the dependency) or an `&'static` reference (when the
dependency is a singleton). The choice is driven by
`metadata.MemoryModel.allocation`: `static_alloc` → `&'static`,
`heap_alloc` or unset → owned field.

`architecture.toml` `[[connections]]` entries do **not** emit code by
themselves — they're verified at validate time but the per-block
codegen describes how to wire them (e.g. embassy uses channels, RTIC
uses shared state).

Do **not** emit a global `Project` struct or context object. Each
module is independent; cross-module wiring is per-block.
