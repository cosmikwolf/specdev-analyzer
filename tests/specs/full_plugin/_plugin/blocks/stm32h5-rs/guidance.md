# stm32h5-rs — implementation guidance

Rules an agent follows when it writes or edits code for a module with an `[stm32h5]` table: STM32H5 peripherals,
pins and interrupts without an async runtime. No check verifies them; review does.

## Scope

- This block's code uses the non-async STM32H5 HAL and PAC, and never an async runtime's HAL types; a bridge owns
  that pairing and its rules win for the peripherals it covers.
- The non-async HAL has no usable release yet. Without the bridge, the spec's topology validates but there is no
  buildable peripheral code; say so rather than invent an API.
- The HAL's MCU feature names the exact family member; it lives in `Cargo.toml`.
- Core, runtime, panic and logger crates are project choices, not this block's.

## Peripherals and pins

- Each `[[stm32h5.peripherals]]` entry is one typed handle, chosen by `kind` and `instance`; module structs own them
  as fields.
- Only the entry point takes the peripheral singleton (it can be taken once) and hands peripherals to module
  constructors.
- DMA on H5 is GPDMA: request-line channels, wired when the peripheral is built — not the older stream model.
- Each `[[stm32h5.pins]]` entry is one typed pin from its GPIO bank, in the declared `mode` (`alt_fn` for
  alternate). GPIO `instance` 1 is bank A, 2 is B, and so on. Split a bank before taking its pins, and declare the
  GPIO peripheral for every bank a module's pins use.
- USB-OTG-FS uses PA11 and PA12 in alternate function 10.

## Outside this block

- Clock-tree setup and the instruction cache are configured once at the entry point, not per module.

## Interrupts

- An interrupt handler is a free function named after its vector, which follows the peripheral's `kind` and
  `instance`. No allocation, blocking I/O or task spawning in a handler.

## HAL and PAC

- Use the HAL by default. Drop to the PAC only for configuration the HAL does not expose.
- Never mix HAL and PAC handles for one peripheral instance, and never mix two PACs for one instance.
