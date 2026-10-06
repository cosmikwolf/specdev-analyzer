# embassy+stm32h5-rs — implementation guidance

Rules an agent follows when it writes or edits code that hands an STM32H5 peripheral into an embassy task. No check
verifies them; review does.

## The join crate

- The embassy-flavoured STM32 HAL belongs to this pairing, not to either block. Its types replace the non-async HAL's
  for every task-owned peripheral.
- Its chip feature is the full part number, exactly one. Use its timer-driver feature that picks any timer, not a
  specific one. Leave the crate's memory-layout feature off: the project owns `memory.x`.
- A separate driver package that borrows a bus type but owns no binding declares this crate itself in its own
  `Cargo.toml`, with the same version and chip feature. Dependencies follow the binding.

## The entry point

- The HAL's `init` runs once, at the entry point only. It replaces the PAC singleton and clock scaffolding; a
  project-specific clock tree goes in its config value. The executor's main attribute is the entry; add no second
  entry attribute.
- Typed handles are built at the entry point and passed by value into the owning module's `init`; raw peripheral
  fields never cross a module boundary.
- Async peripherals bind their interrupts once, at the crate root: the peripheral's vectors and each DMA channel's.
  No raw interrupt handler for a vector that is also bound there.
- Check constructor argument order, return types (some return a result, some do not) and generic arity against the
  pinned HAL. Bus masters take a third generic parameter.
- Output and input pins become GPIO handles. Alternate-function bus pins go into the bus constructor as raw peripheral
  singletons, never wrapped as GPIO.

## Bindings

- Each `peripheral_tasks[]` entry binds one embassy task to one peripheral, both on the same module. So the module
  that owns the task owns the peripheral, pins and binding; a reusable driver stays pure logic that borrows the
  built handle as a parameter.
- `handoff = "owned"` (default): the task takes the handle by value. Never wrap it in a mutex.
- `handoff = "shared"`: the handle lives in one static async mutex holding an empty slot, filled once by `init`
  before any task spawns; tasks lock it across awaits. Never a `RefCell`.
