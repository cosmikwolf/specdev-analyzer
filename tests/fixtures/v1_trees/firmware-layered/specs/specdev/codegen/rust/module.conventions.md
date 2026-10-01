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

**Crate root — binary vs library member.** The crate root form follows
`spec.toml`'s `[project].crate_shape`:

- Default / `crate_shape = "binary"` → the crate is an embedded binary. The
  entry point is `src/main.rs` (`#![no_std]` + `#![no_main]`, the runtime's
  entry attribute, and any `bind_interrupts!` the architecture/bridge prose
  prescribes). This is the app package of a workspace.
- `crate_shape = "library"` → the crate is a `no_std` **library** member
  (a driver / HAL-shim borrowed by a sibling crate). The crate root is
  `src/lib.rs`: `#![no_std]`, `mod <layer>;` for each populated layer, and
  `pub use` re-exports of the public items other crates consume. Emit **no**
  `#![no_main]`, no entry attribute, no `bind_interrupts!`, and no panic
  handler — those belong to the binary that links this library. The
  Cargo.toml `[lib]` target is rendered automatically from `crate_shape`.

**Cross-package coupling — Cargo path dep, never `depends_on`.** When a
module depends on a module that lives in a *different* package of the
workspace (e.g. the app package borrowing a driver from a `crate_shape =
"library"` member), express the coupling at the **Cargo layer only**: add
the producing crate to the *consuming* package's `architecture.toml` as
`[[external_deps]]` with `source = { path = "../<crate>" }`, and name the
imported symbol in the consuming module's item/action prose (e.g.
"constructs the `sensor_driver::Dps310` driver"; crate name
`sensor-driver` → import path `sensor_driver`). Do **not** reach for
`depends_on` here — `depends_on` is **intra-package** (it resolves only
against the same package's `specs/modules/` set), so a cross-package
`depends_on` dangles. Each package has its own independent
`specs/specdev/`; there is no workspace-level spec state, and cross-crate
wiring belongs to Cargo, not to a spec relation.

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

## Const tables and `build.rs`

When a module's body computes a non-trivial lookup table — anything where
the entries are derived from a formula (exponentials, log curves, ratio
tables, detune/PN sequences) rather than a handful of named constants —
prefer **`build.rs` table generation** over `const fn` evaluation:

```rust
// build.rs
use std::{env, fs, path::Path};

fn main() {
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("eg_increment.rs");
    let mut entries = Vec::with_capacity(256);
    for i in 0..256u32 {
        let v = compute_eg_increment(i);
        entries.push(format!("    {v},"));
    }
    let body = format!(
        "pub static EG_INCREMENT: [u32; 256] = [\n{}\n];\n",
        entries.join("\n")
    );
    fs::write(out, body).unwrap();
    println!("cargo:rerun-if-changed=build.rs");
}
```

```rust
// src/eg_increment.rs (or wherever the table is consumed)
include!(concat!(env!("OUT_DIR"), "/eg_increment.rs"));
```

Why over `const fn`:
- `Ord` / floating-point ops on associated `const`s are still unstable
  in stable Rust; computing the table at build time avoids the trait
  surface entirely.
- Intermediate overflow on `u32` / `i32` arithmetic is easier to reason
  about in a regular `fn` than under `const`-context constraints.
- Compile-time evaluation of large `const fn` table builders blows up
  build times on cold caches; `build.rs` runs once per build directory.

Do **not** put the table values in the module spec — the spec records
the *shape* (array length, element type, that the table is computed
from a formula) and points the implementer at `build.rs`. Anchor-point
fidelity (matching documented values at e.g. index 0, 64, 128, 255)
should be verified by a `#[test]` in the module, not by hand-checking
the spec.

For trivial constants (single named values, small enums-as-numbers),
keep them inline as `pub const NAME: T = ...;` — `build.rs` is for
*tables*, not all constants.

## Item emission

`module.items[]` is an OPTIONAL surface in firmware-layered (firmware
modules' contractual surface lives mostly in `actions[]`, `ports[]`,
`state_machines[]`, and `errors[]` rather than in items[]'s public-API
shape). When present, items[] entries carry top-level Rust constructs
worth tracing to requirements — most often `pub` const tables (driver
calibration data), `pub` static peripheral handles, or trait impls
that fulfill a `verified_by` contract.

The shape is **prose-first**: each item's `description` summarizes what
the construct contractually provides, NOT how it's typed. The optional
`signature_hint` field exists as an escape hatch for FFI declarations,
macro contracts, and other cases where prose alone is insufficient;
avoid it for ordinary firmware functions.

Sparseness rule: every item except `kind = "re_export"` must carry
`satisfies` (≥1 requirement id). The schema enforces this. For
firmware-layered, items[] is most useful for:

- `kind = "const"` calibration tables that satisfy a measured-behavior
  requirement (`["DRV-CALIB-003"]`).
- `kind = "fn"` ISR handlers whose timing budget is a requirement.
- `kind = "impl"` trait impls that satisfy a contractual interface
  required by upper-layer code.

An item is identified by its **fully-qualified path**, prose-first, never
a signature. A method is a function: if you do author a public inherent
method, it is a `kind = "fn"` item named `<Type>::<method>` (generics
normalized out of the prefix) — the inherent `impl` block itself is not an
item. A trait impl stays one `kind = "impl"` item (`<Trait> for <Type>`);
its method bodies are codegen-derived, not separate items. (Most firmware
methods come from `actions[]` below, not items[].)

When firmware module specs are content-rich in actions/ports/state
machines, items[] often stays empty — that's correct. items[] is a
**supplemental** traceability surface for firmware-layered, not a
replacement for the structural fields.

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

`depends_on[]` is **intra-package only** — every entry resolves against
this package's own `specs/modules/`. For a dependency that lives in
*another* workspace package, do not use `depends_on` (it will dangle);
see "Crate root — binary vs library member" above for the Cargo
path-dep-plus-name-the-symbol rule.

`architecture.toml` `[[connections]]` entries do **not** emit code by
themselves — they're verified at validate time but the per-block
codegen describes how to wire them (e.g. embassy uses channels, RTIC
uses shared state).

Do **not** emit a global `Project` struct or context object. Each
module is independent; cross-module wiring is per-block.

## Project-wide infrastructure

The spec is the source of truth for every Cargo dependency. `gen` reads
`architecture.toml`'s `[platform]`, `[runtime]`, `[logger]`, and
`[[external_deps]]` sections (plus `spec.toml`'s `[project]`) and emits
the complete Cargo project, not just module bodies. **Do not add deps
to Cargo.toml that aren't declared in the spec; do not omit deps that
are declared.**

### `[platform]` drives target + ARM core scaffolding

- `[platform].target_triple` → Cargo build target (`.cargo/config.toml`
  `build.target` and `[target.<triple>.rustflags]` for `-C link-arg=-Tlink.x`).
- `[platform].mcu_part` → MCU block consumes this to pick its HAL
  feature flag (e.g. stm32h5-rs reads `mcu_part` to select
  `stm32h5xx-hal`'s `stm32h563` feature). If `mcu_part` is set but no
  MCU block is inlined, emit a warning comment in `main.rs`:
  `// SPEC WARNING: [platform].mcu_part = "<value>" but no MCU block found`.
- `[platform].mcu_core` → `[dependencies]` (typically `cortex-m`).
- `[platform].mcu_runtime` → `[dependencies]` (typically `cortex-m-rt`).
  `main.rs` gets `#[cortex_m_rt::entry]` on the entry point.

### `[runtime]` drives panic + allocator + critical-section setup

- `[runtime].panic_handler` → `[dependencies]` AND `main.rs` gets
  `use <crate-as-snake_case> as _;` to register the handler. For
  `panic-probe`, emit `use panic_probe as _;`.
- `[runtime].allocator` (optional) → `[dependencies]` AND a
  `#[global_allocator]` static at crate root. Add `extern crate alloc;`
  at crate root regardless of any module's `MemoryModel.allocation`
  setting. If absent, do **not** emit `extern crate alloc;` from this
  rule (modules with `heap_alloc` still trigger it per the section
  above).
- `[runtime].critical_section_impl` (optional) → `[dependencies]`. Do
  not name a specific crate; the embassy or stm32 block's prose covers
  the integration pattern when relevant.

### `[logger]` drives logging integration

- `[logger].provider` → `[dependencies]` (typically `defmt`).
- `[logger].backend` → `[dependencies]` AND `main.rs` gets
  `use <backend-as-snake_case> as _;` (for defmt-rtt: `use defmt_rtt as _;`).
- `[logger].default_level` → emit a `defmt::set_log_level!` call (or
  equivalent) at the start of the entry point, when the provider supports
  runtime level setting; otherwise document in a comment.

### `[[external_deps]]` is appended verbatim

Every entry lands in Cargo.toml `[dependencies]` with its `crate`,
`version`, optional `features`, optional `default-features`, optional
`source` (path/git/registry). No code is emitted from these — the
project author already knows how to use the crate; the spec just
records what it is so the build is reproducible.

### `[[features]]` → `[features]`

Each `[[features]]` entry in `architecture.toml` lands in Cargo.toml
`[features]`, in name order, after the crate target (and any `[[test]]`
rows) and before `[dependencies]`. The section always opens with a
synthesized `default = [...]` line listing the entries marked
`default = true` (empty when none). A feature's line carries its
`depends_on` entries and its `cargo_deps` strings, sorted; a feature
with neither is a plain cfg switch and renders `name = []`. A
`cargo_deps` entry of the form `dep:<crate>` enables a dep whose
CrateReference carries `optional = true` — that dep renders with
`optional = true` in `[dependencies]`, and Cargo refuses a `dep:` that
names a non-optional crate (Cargo's error; the spec does not re-check
it). Every `#[cfg(feature = "...")]` site in a module must name a
declared feature.

### `[[test_targets]]` → `[[test]]` rows

Each `[[test_targets]]` entry in `architecture.toml` lands in Cargo.toml
as one `[[test]]` row, in declaration order, after the crate target and
before `[dependencies]`: `name`, then `path` when the spec gives one,
then `harness = false` only when the spec says so (Cargo's default is
`true`, so it is never written). Cargo still auto-discovers
`tests/<name>.rs` targets the spec does not list; list a target only
when it needs a non-default setting. A test target is the binary that
hosts test items; it is not a test item and carries no `verifies[]`.

### `[profiles.<name>]` → `[profile.<name>]` (standalone crate only)

A standalone firmware crate's build profiles come from `architecture.toml`
`[profiles.<name>]` and land after `[dependencies]` as `[profile.<name>]`
tables — dev, release, test, bench in that order, then custom profiles
lex-sorted, keys in a fixed order, each `[profile.<name>.package.<crate>]`
override right after its parent with crates lex-sorted. The subset is
closed (`opt-level`, `debug`, `lto`, `codegen-units`, `panic`, `strip`,
`overflow-checks`, `debug-assertions`, `incremental`; overrides without
`panic` / `lto`). In a workspace the profiles belong to the root's
architecture.toml: Cargo ignores them in a member manifest, `validate`
rejects them on a member (`PROFILES-ROOT-ONLY`), and the member render
emits nothing for them.

### Determinism

Cargo.toml entries emit in lex-order by crate name. Within `[features]`
emit features in lex-order. The same spec must produce byte-identical
Cargo.toml across runs.
