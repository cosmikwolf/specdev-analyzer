# specdev-analyzer

Domain-agnostic CLI that turns TOML specs into first-class engineering artifacts: schema-validated, graph-checked, extractable to codegen-ready YAML, and auditable against real source code.

Pairs with the [`specdev`](https://github.com/cosmikwolf/specdev) Claude Code plugin, but works standalone — `cargo install`-able and has no runtime dependency on the plugin at analyze time (doc 15 §11 invariant 5: no resolution chain at analyze time).

## Status — Phase 2 of the blocks + bridges + profiles arc

This branch ships **Phase 2**: the metaschema amendment (path-qualified kinds + required `extends`), the open-vocabulary effective kind set, the compiled aggregated module schema, and the executor + extract + audit chain end-to-end. Canonical root-kind and relation-kind enums are embedded in the analyzer (doc 15 §11 invariant 5: no plugin lookup at analyze time). Subcommand behavior:

- **`validate`** — discovers `spec.toml`, builds a layout context, enumerates the `specs/specdev/` tree (`LAYOUT-002`/`LAYOUT-003`/`LAYOUT-004`), runs the aggregator `$ref` resolver and compiles the aggregated module schema (`AGGREGATOR-001` unresolved / `AGGREGATOR-002` duplicate-`$id` / `AGGREGATOR-003` cycle / `AGGREGATOR-005` compile-fail), resolves the open-vocabulary kind set from project + block + bridge reductions (`METASCHEMA-001..005`), validates each module TOML against the compiled schema (`SCHEMA`), and runs the reduction executor and `domain.toml` graph_checks against the resulting graph (`REDUCE_UNRESOLVED` / `DOMAIN-003`).
- **`extract`** — runs validate, then the executor, and writes canonical YAML under `--output`: `modules/<Name>.yaml` (one per source module), `architecture.yaml` (full executor graph), `requirements.yaml` (entities whose `extends` chain reaches `statement`). Output is byte-stable across runs.
- **`reduce`** — runs the executor and emits the `(entities, relations)` graph YAML to stdout or `--output`.
- **`audit`** — validate → extract → tree-sitter compare via the configured language adapter (Rust, C, C++).
- **`resolve`** — prints the discovered project layout as YAML for skill consumers.

Errors carry a structured `<path>:<line>:<col>: <message>` payload via `LocatedError` so editors and CI can jump to the failing site.

## Install

```
cargo install --git https://github.com/cosmikwolf/specdev-analyzer
```

Requires a stable Rust toolchain (MSRV 1.75).

## Usage

```
specdev-analyzer validate [--project <dir>] [--strict] [--no-color]
specdev-analyzer extract  --output <dir> [--project <dir>]
specdev-analyzer audit    [--project <dir>] [--filter <Module>] [--show-uncovered] [--expand]
specdev-analyzer reduce   [--project <dir>] [--output <path>]
specdev-analyzer resolve  [--project <dir>]
```

Every subcommand also accepts `--spec-path <path>` to short-circuit `spec.toml` discovery.

`--project` defaults to the current directory. The project root is the directory containing `spec.toml`. The CWD walk discovers `spec.toml` automatically.

Set `RUST_LOG=specdev_analyzer=info` for progress events, `RUST_LOG=specdev_analyzer=debug` for per-file enumeration.

### `spec.toml` shape

```toml
spec_version = "1.0"

[project]
name = "my-firmware"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
```

Validated against an embedded JSON Schema at parse time. The analyzer is the source of truth for `spec.toml`; nothing in the schema comes from the plugin.

### Project layout (single-package)

```
my-firmware/
├── spec.toml                                   # project manifest
├── src/                                        # source code (audit / reverse target)
└── specs/
    ├── modules/*.toml                          # user-authored module specs
    ├── architecture.toml                       # cross-module wiring (Phase 2+)
    ├── requirements.toml                       # requirements (Phase 2+)
    └── specdev/                                # CLI-managed machinery
        ├── domain.toml                         # project graph-check declarations
        ├── lineage.toml                        # provenance for inlined units
        ├── schemas/
        │   ├── module.schema.yaml              # aggregator: allOf $refs to inlined/
        │   └── inlined/
        │       ├── blocks/<name>.schema.yaml   # one per inlined block
        │       └── bridges/<name>.schema.yaml  # one per inlined bridge
        ├── reductions/
        │   ├── project.yaml
        │   ├── blocks/<name>.yaml
        │   ├── bridges/<name>.yaml             # canonical bridges
        │   └── bridges/local/<name>.yaml       # project-local bridges
        └── codegen/<lang>/
            ├── module.conventions.md
            ├── blocks/<name>/README.md
            └── bridges/<name>/README.md
```

Multi-package projects are per-package independent under doc 15 §9.2: each package owns its own `spec.toml` and `specs/specdev/` tree. There is no workspace-level `[[packages]]` table.

## Library API (Phase 1)

The crate exports a small library surface for tools that want to drive the same plumbing. See `src/lib.rs` for the entry-point map. Phase 2 will reintroduce a contract test against `specdev-visualizer`'s consumption shape.

## License

MIT. See `LICENSE.md`.
