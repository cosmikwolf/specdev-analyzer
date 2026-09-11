# stm32h5-rs block

STM32H5 MCU family block targeting [`stm32h5xx-hal`](https://github.com/stm32-rs/stm32h5xx-hal)
and [`stm32h5xx-pac`](https://docs.rs/stm32h5xx-pac). Contributes a
namespaced `[stm32h5]` table on `Module` with two concerns:
**peripherals** (USART/UART/SPI/I2C/GPIO/TIM/ADC/DMA/USB/OCTOSPI/RCC by
kind + instance) and **pins** (GPIO port + pin + alternate-function
mapping).

Default verification target: STM32H563 on the Nucleo-H563ZI dev board.

## When to use this block

Inline `stm32h5-rs` when your project:

- Targets an STM32H5 family MCU (H503, H523, H533, H562, H563, H573).
- Uses the [`stm32h5xx-hal`](https://github.com/stm32-rs/stm32h5xx-hal)
  crate for typed peripheral handles.
- Authors per-module hardware ownership rather than a monolithic
  `main.rs` with all peripherals.

Don't use this block if:

- You're on a different STM32 family (F1/F4/L4/H7/G4/U5 etc.). Each
  warrants its own block (`stm32f4-rs`, `stm32g4-rs`, …) — codegen
  mappings differ.
- You're using `embassy-stm32` directly. The `embassy-stm32` crate is
  embassy-flavored and would conflict with stm32h5-rs's HAL type
  emissions; the `embassy+stm32h5-rs` bridge handles that integration.
- You're writing PAC-only firmware (no HAL). The HAL types are the
  contract this block emits against.

## What this block contributes

Schema additions on `Module`:

```toml
[[stm32h5.peripherals]]
name = "uart_dev"
kind = "USART"
instance = 3            # USART1, USART2, USART3, ...

[[stm32h5.pins]]
name = "tx_pin"
port = "D"
pin = 8                 # 0..15
alt_fn = 7              # alternate-function number per H563 datasheet
mode = "alternate"
```

Reduction kinds (after inline):

- `stm32h5-rs.Peripheral` (extends `Module`) — one per
  `[[stm32h5.peripherals]]`. Qualified name is
  `<ModuleName>.<peripheral_name>`.
- `stm32h5-rs.Pin` (extends `Module`) — one per `[[stm32h5.pins]]`.
  Qualified name same shape.

Codegen prose (`codegen/rust/README.md`) maps each shape to typed Rust:
peripherals to `Serial<USART3>` / `Spi<SPI2>` / etc., pins to
`gpiod.pd8.into_alternate::<7>()`, ISRs to `#[interrupt]`-attributed
free functions.

Closed enum for `kind`: `USART | UART | SPI | I2C | GPIO | TIM | ADC |
DMA | RCC | USB | OCTOSPI`. v1.x can widen as projects need (LPUART,
ETH, RNG, CRC, HASH, AES, LPTIM).

## Sharp-test rationale

- **Concreteness:** ✓ — names a specific MCU family
  (`stm32h5xx-hal` / `stm32h5xx-pac`), not the abstract concept of
  "MCU peripherals."
- **Codegen-completeness:** ✓ — given a TOML with N peripherals + M
  pins, prose emits N typed peripheral handles + M typed pin handles +
  any necessary GPIO bank splits + interrupt-handler scaffolds. The
  HAL is maturing; some peripherals (USB device classes, crypto)
  require PAC-fallback documented as an escape hatch.
- **No cross-block reach:** ✓ — codegen explicitly excludes
  `embassy-stm32` and other runtime-flavored HAL crates. Embassy
  integration is bridge territory (`embassy+stm32h5-rs`).

## Deferred (revisit triggers per §12)

- **Clock-tree configuration** — HSE/HSI/PLL1/PLL2/PLL3/peripheral-
  clock setup is one-per-project, not one-per-module. Either lives in
  `specs/architecture.toml` (firmware-layered profile extension) or in
  a future `stm32h5-clocks` block.
- **GPDMA channel allocation** — GPDMA peripherals are declared via
  `kind = "DMA"` but channel-to-peripheral request line mapping (e.g.
  "GPDMA1 channel 3 serves USART3_RX") is implicit in HAL construction
  and codegen. A typed `[[stm32h5.dma_channels]]` entry could promote
  this; defer until a real project hits the wall.
- **Interrupt priority** — currently free-form per peripheral
  declaration. NVIC priority assignment could become a typed field
  when prioritization across modules becomes a real concern.
- **TrustZone configuration** — STM32H5 supports ARMv8-M TrustZone.
  Setup is project-wide, not per-module. Defer until a project needs
  secure/non-secure split; v1 assumes non-secure-only operation.
- **ICACHE / DCACHE** — instruction/data caches are configured at boot,
  project-level. Document at architecture level.
- **Other STM32 families** — F1/F4/L4/H7/G4/U5/WB/WL each warrant their
  own block. The conceptual shape (peripherals + pins) overlaps but
  the kind enums and HAL type mappings differ enough to ship as
  separate blocks.
- **PAC-direct register access** — codegen prose mentions PAC as an
  escape hatch. A typed `[[stm32h5.raw_registers]]` entry could capture
  this if multiple projects converge on similar low-level patterns; v1
  leaves it informal.

## History

This block replaces an earlier `stm32f4-rs` block (dropped 2026-04-30)
when the user's dogfood target shifted to STM32H5 hardware. F4 support
could be re-authored as a separate `stm32f4-rs` block if a future
project needs it; this block stands as a parallel reference.
