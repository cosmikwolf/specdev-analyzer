# mk20dx — implementation guidance

Rules an agent follows when it writes or edits code for a module with an `[mk20dx]` table: the MK20DX (Teensy 3.x)
chip's pins, interrupts, clock and peripherals through its HAL. The rule pack checks that no two pin entries claim the
same port and pin (`mk20dx.pins_unique`); review checks the rest.

## Facts never to guess

- An `alt_fn` mux index, a DMA `source`, and the flag-clear sequence of a peripheral other than ports, PIT and DMA are
  part facts. If the spec does not settle one, stop and ask: a guess compiles and then fails on hardware.
- The PAC comes through the HAL's feature; never name a PAC crate (the bare crates.io name is an unrelated crate).

## Splits and handles

- Ports, the PIT and the DMA controller are each split once, in the project's init path, and the parts are handed to
  the modules that own them. A port split also takes the SIM reference that clocks it.
- A pin handle is named after the spec's `name` (its role), never its coordinates; a stored pin carries its full
  typed mode so a wrong mode fails at the field. The alternate-function mux is a type parameter, not an argument.

## Interrupts

- Every handler clears its source's flag before returning; otherwise it re-enters at once and the program seems to
  hang.
- One handler per port with interrupting pins, never one per pin. It reads the port's flag register, dispatches by
  bit, and writes back exactly the value it read — never a blanket clear, which loses a flag that arrived meanwhile.
- A level-triggered pin's handler removes the level (service the device or disable the trigger), not only the flag.
- A `dma_*` pin trigger raises a DMA request; it gets no handler.
- When the trigger is set, the PCR's write-1-to-clear flag bit is written as 0, so configuring does not clear a
  pending flag.
- This block says which vector and what to clear; the concurrency runtime attaches the handler. Handlers latch and
  hand off; no long work.
- For a runtime that needs them: the device path is the HAL crate itself, not the PAC; dispatcher vectors come from
  peripherals the design does not use (the CAN vectors on boards without CAN), never a vector a module declares.

## Clock

- At most one module declares `[mk20dx.clock]`; refuse to proceed if two do. The clock is frozen once, before any
  peripheral, and every constructor takes it.
- `mhz48` is the smaller part's shape. A preset that does not fit the part is a mismatch to raise, not to round. Do
  not downgrade an overclock: it is the board's recorded choice.

## Peripherals

- PIT: an entry with `period_us` is started; one without is handed over unstarted. Its interrupt is enabled only
  when the module also declares the `PIT<n>` vector; otherwise poll.
- SPI and UART take `alternate`-mode pins from the module's own `pins`; name the SPI instance marker by its full path
  (the PAC type has the same name). The baud is a target the hardware rounds down; no compensation.
- UART: split TX and RX only when two owners need them.
- ADC: set resolution and averaging only when given; calibrate unless `calibrate = false`, and handle its failure.
- DMA: an entry with `source` is wired at init; one without is software-triggered.
- FTM: with `frequency_hz`, start PWM and enable each declared channel (duty 0 unless the module says otherwise; a
  single channel's duty is its compare value). Without it, hand the parts over stopped.
- I2C, I2S, USB, CMP, VREF and DAC are outside this block.
