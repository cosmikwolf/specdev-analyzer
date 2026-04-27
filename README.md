# specdev-analyzer

Domain-agnostic CLI that turns TOML specs into first-class engineering artifacts: schema-validated, graph-checked, extractable to codegen-ready YAML, and auditable against real source code.

Pairs with the [`specdev`](https://github.com/cosmikwolf/specdev) Claude Code plugin, but works standalone — `cargo install`-able and has no runtime dependency on the plugin at analyze time (doc 15 §11 invariant 5: no resolution chain at analyze time).

## Status — Phase 1 of the blocks + bridges + profiles arc

This branch ships **Phase 1**: layout discovery, the aggregator `$ref` resolver, the reduction loader, and a Phase-1 stub for module structural validation. Subcommands and current behavior:

- **`validate`** — discovers `spec.toml`, builds a layout context for `specs/specdev/`, enumerates every expected directory and file (`LAYOUT-002`/`LAYOUT-003`/`LAYOUT-004`), runs the aggregator `$ref` resolver (`AGGREGATOR-001` unresolved-ref / `AGGREGATOR-002` duplicate-`$id` / `AGGREGATOR-003` cycle), loads project / block / bridge reductions (`REDUCTION-001`), and emits one `LAYOUT-005` info per module discovered under `specs/modules/` (structural module validation lands in Phase 2).
- **`extract`**, **`audit`**, **`reduce`** — Phase-1 stubs. Each bails with a clear "available in Phase 2 once kind resolution lands" message. Phase 2's metaschema amendment (path-qualified kinds + `extends`) wires them in.
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
specdev-analyzer extract  --output <dir> [--project <dir>]   # Phase 2 stub
specdev-analyzer audit    [--project <dir>] [--filter <Module>] [--show-uncovered] [--expand]   # Phase 2 stub
specdev-analyzer reduce   [--project <dir>] [--output <path>]   # Phase 2 stub
specdev-analyzer resolve  [--project <dir>]
```

Every subcommand also accepts `--spec-path <path>` to short-circuit `spec.toml` discovery.

`--project` defaults to the current directory. The project root is the directory containing `spec.toml`. The CWD walk discovers `spec.toml` automatically.

Set `RUST_LOG=specdev_analyzer=info` for progress events, `RUST_LOG=specdev_analyzer=debug` for per-file enumeration.

### `spec.toml` shape (Phase 1)

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
