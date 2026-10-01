# Frozen `1.x` project trees

Byte copies of what `specdev-analyzer bootstrap` (analyzer 0.2.x, the released
`spec_version = "1.0"` format) wrote from the test plugin on 2026-09-30, one per
profile, plus — in `library-api/` — one generated file recorded by `gen-record`
(`src/built.rs` in `specs/specdev/gen-lineage.toml`).

They are the input side of the `migrate` tests: a tree as the old tools left it,
with combined ledgers, an inlined `codegen/` directory, `gen-lineage.toml` and a
`domain.toml` naming `codegen_unblocked`. **Do not regenerate them with newer
tools** — a newer bootstrap writes the new format, and the tests would then
migrate nothing. Tests copy a tree into a temporary directory before use.
