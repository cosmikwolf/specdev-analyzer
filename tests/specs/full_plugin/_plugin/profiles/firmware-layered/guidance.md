# firmware-layered — implementation guidance

Rules an agent follows when it writes or edits code in a `firmware-layered` package: a layered `no_std` firmware
crate. Framework rules come from the inlined blocks' guidance. No check verifies these rules; `check` verifies the
layers and the imports, and review does the rest.

## Crate and files

- One file per module, under its layer's directory (`src/<layer>/<name>.rs`), so the path shows the layer. The crate
  root declares the populated layers.
- A binary crate's root is `src/main.rs`: `no_std`, `no_main`, the runtime's entry, the interrupt bindings that the
  units prescribe, and the panic handler. A library member's root is `src/lib.rs`: `no_std` and re-exports only — no
  entry, no interrupt bindings, no panic handler.
- `alloc` only when an allocator is set or a module's memory model is `heap_alloc`. No blanket lint suppressions. No
  serialization derives by default: they bloat `no_std` builds.
- `Cargo.toml` and `.cargo/config.toml` are the truth for dependencies, features, build target and profiles.

## Modules

- A module is a struct with its actions as methods on it; no free functions for actions, no global project struct.
- `blocking = false` actions are async unless a unit prescribes another idiom. `isr_safe = true` actions call no
  blocking primitive and say so in a safety comment.
- Each state machine is a fieldless state enum with a step function over the spec's transitions only; any other pair
  leaves the state unchanged.
- `metadata.MemoryModel.allocation`: `static_alloc` keeps state in the struct, with no heap; `heap_alloc` allows
  owned heap data. It also decides whether a dependency is held as a `'static` reference or as an owned field.
- `metadata.ErrorHandling.strategy` decides how actions report failure, one strategy per module: a result with the
  module's error enum by default; `panic` only on HAL-layer init paths.
- Hardware pins and peripherals live on the module that declares them; their concrete types come from the chip unit,
  never named by family here.

## Dependencies between modules

- `depends_on` is within one package. A module in another workspace package is reached as a Cargo path dependency,
  and the importing module's prose names the symbol; never a cross-package `depends_on`.
- `[[connections]]` are checked by `validate`; how they are wired (channels, shared state) is the units' guidance.

## Tables

- A lookup table computed from a formula is generated in `build.rs` and included, not built with a `const fn`. The
  spec records its shape; a test checks anchor values. Plain named constants stay inline.
