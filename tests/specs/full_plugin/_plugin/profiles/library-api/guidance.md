# library-api — implementation guidance

Rules an agent follows when it writes or edits code in a `library-api` package: a Rust library whose public surface
is its contract. No check verifies these rules; `check` verifies the imports and the tests, `items diff` the
contract surface, and review the rest.

## Files and visibility

- A top-level module is `src/<name>.rs`; a child is `src/<parent>/<child>.rs`, declared from `src/<parent>.rs`, and
  its spec sets `source_file` (`check` finds only `src/<name>.rs` and `src/<name>/mod.rs` by name). Use a `mod.rs`
  only when the parent itself spans several files.
- `module.visibility` decides the declaration in the parent: `pub`, `crate` (crate-visible) or `private`. Inside a
  crate-visible module, items are plain `pub`; the module already limits their reach.
- Import siblings by explicit path; no glob imports from the parent.

## The contract surface

- When the module keeps `items[]`, each public symbol is one item named by its full path (`Type::method` for a
  public method, `Trait for Type` for a contractual trait impl), described in prose — never by signature or generics.
  Private helpers are not items.
- A public symbol missing from `items[]` shows as source-only in the items sync: author it with a `satisfies`, or
  make it private.
- A `re_export` item is one `pub use` (or crate-visible use), renamed when its `name` differs from the target's last
  segment.
- Public types derive `Debug`; cheap value types derive `Clone`; `Default` only when a default means something. Keep
  helpers private until a real consumer needs them.

## no_std and features

- A library with a `std` feature is `no_std` unless that feature is on (conditional attribute, not an unconditional
  `no_std`); `alloc` is declared behind its feature.
- Every `cfg(feature = …)` names a feature that `Cargo.toml` declares. Feature and target conditions are separate
  attributes.
- `Cargo.toml` is the truth for dependencies, features and profiles; test-only crates are dev-dependencies. In a
  workspace, profiles belong to the root manifest.

## Tests

- A requirement's `verified_by` names the test by the identity that `specdev-analyzer report tests` shows. A unit
  test sits in the module's test module, an integration test in `tests/`.
- An unfinished test fails loudly and names the requirement it owes; it never passes empty.

## Tables

- A lookup table computed from a formula is generated in `build.rs` and included; a test checks anchor values. Plain
  named constants stay inline.
