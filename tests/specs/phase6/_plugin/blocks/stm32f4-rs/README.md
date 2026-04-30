# stm32f4-rs block

STM32F4 MCU family block targeting [`stm32f4xx-hal`](https://docs.rs/stm32f4xx-hal)
and [`stm32f4xx-pac`](https://docs.rs/stm32f4xx-pac). Contributes a
namespaced `[stm32f4]` table on `Module` with two concerns:
**peripherals** (USART/SPI/I2C/GPIO/TIM/etc., by kind + instance) and
**pins** (GPIO port + pin + alternate-function mapping).

## When to use this block

Inline `stm32f4-rs` when your project:

- Targets an STM32F4 family MCU (F401, F411, F405, F407, F415, F417,
  F427, F429, F437, F439, F446, F469, F479).
- Uses the [`stm32f4xx-hal`](https://github.com/stm32-rs/stm32f4xx-hal)
  crate for typed peripheral handles.
- Authors per-module hardware ownership rather than a monolithic
  `main.rs` with all peripherals.

Don't use this block if:

- You're on a different STM32 family. STM32H7 / F1 / L4 each need
  their own block (`stm32h7-rs`, etc.) — codegen mappings differ.
- You're using `embassy-stm32` directly. The `embassy-stm32` crate is
  embassy-flavored and would conflict with stm32f4-rs's HAL type
  emissions; the `embassy+stm32f4-rs` bridge (Phase 7+) handles that
  integration.
- You're writing PAC-only firmware (no HAL). The HAL types are the
  contract this block emits against.

## What this block contributes

Schema additions on `Module`:

```toml
[[stm32f4.peripherals]]
name = "uart_dev"
kind = "USART"          # USART | SPI | I2C | GPIO | TIM | ADC | DMA | RCC
instance = 1            # USART1, USART2, etc.

[[stm32f4.pins]]
name = "tx_pin"
port = "A"              # A | B | C | D | E | F | G | H
pin = 9                 # 0..15
alt_fn = 7              # alternate-function number per MCU datasheet
mode = "alternate"      # input | output | alternate | analog
```

Reduction kinds (after inline):

- `stm32f4-rs.Peripheral` (extends `Module`) — one per
  `[[stm32f4.peripherals]]`. Qualified name is
  `<ModuleName>.<peripheral_name>`.
- `stm32f4-rs.Pin` (extends `Module`) — one per `[[stm32f4.pins]]`.
  Qualified name same shape.

Codegen prose (`codegen/rust/README.md`) maps each shape to typed Rust:
peripherals to `Serial<USART1>` / `Spi<SPI2>` / etc., pins to
`gpioa.pa9.into_alternate::<7>()`, ISRs to `#[interrupt]`-attributed
free functions.

## Sharp-test rationale (doc 15 §3.1)

- **Concreteness:** ✓ — names a specific MCU family
  (`stm32f4xx-hal` / `stm32f4xx-pac`), not the abstract concept of
  "MCU peripherals."
- **Codegen-completeness:** ✓ — given a TOML with N peripherals + M
  pins, prose emits N typed peripheral handles + M typed pin handles +
  any necessary GPIO bank splits + interrupt-handler scaffolds. No
  step requires "depends on which X."
- **No cross-block reach:** ✓ — codegen explicitly excludes
  `embassy-stm32` and other runtime-flavored HAL crates. Embassy
  integration is bridge territory.

## Deferred (revisit triggers per §12)

- **Clock-tree configuration** — HSE/HSI/PLL/peripheral-clock setup is
  one-per-project, not one-per-module. Either lives in
  `specs/architecture.toml` (firmware-layered profile extension) or in
  a future `stm32f4-clocks` block.
- **DMA stream pairing** — currently DMA peripherals are declared via
  `kind = "DMA"` but stream-to-peripheral mapping (e.g. "DMA1 stream 5
  serves USART1 RX") is implicit in the codegen. A typed
  `[[stm32f4.dma_streams]]` entry could promote this; defer until a
  real project hits the wall.
- **Interrupt priority** — currently free-form per peripheral
  declaration. NVIC priority assignment could become a typed field
  (`priority: 0..15` on Peripheral) when prioritization across
  modules becomes a real concern.
- **Other STM32 families** — F1 / L4 / H7 / WB etc. each warrant
  their own block (`stm32f1-rs`, `stm32l4-rs`, ...). The conceptual
  shape (peripherals + pins) overlaps but the kind enums and HAL
  type mappings differ enough to ship as separate blocks.
- **PAC-direct register access** — the codegen prose mentions PAC as
  an escape hatch. A typed `[[stm32f4.raw_registers]]` entry could
  capture this if multiple projects converge on similar low-level
  patterns; v1 leaves it informal.
