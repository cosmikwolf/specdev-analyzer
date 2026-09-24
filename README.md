# specdev-analyzer

Domain-agnostic CLI that turns TOML specs into first-class engineering artifacts: schema-validated, graph-checked, extractable to codegen-ready YAML, and auditable against real source code.

Pairs with the `specdev` Claude Code plugin, but works standalone — `cargo install`-able, with no
runtime dependency on the plugin at analyze time (doc 15 §11 invariant 5: no resolution chain at
analyze time).

## What it does

Five things, in the order a project meets them:

| | |
|---|---|
| **Scaffold** | `bootstrap` lays down a profile's spec tree; `inline` / `inline-bridge` pull in blocks and the bridges that wire them together; `migrate` back-fills manifests for projects that pre-date them. |
| **Check** | `validate` resolves the open-vocabulary kind set, compiles the aggregated module schema, and runs the project's own `domain.toml` graph checks. `plugin-lint` checks a plugin tree instead of a project. |
| **Project** | `reduce` emits the universal entity/relation graph; `extract` writes codegen-ready YAML; `context` cuts a one-hop dependency cone for one module; `report decisions` shows which decision text reaches which module's codegen prompt, and why; `report graph` renders the whole module graph as Mermaid or DOT, grouped by the project's declared layers, with every edge the layer check flags drawn red; `resolve` and `report` answer layout and coverage questions. |
| **Generate** | `gen` assembles the codegen prompt (shared preamble once, then a tail per module); `manifest` renders the build manifest; `derive` writes the profile's derived files. |
| **Keep honest** | `gen-record` + `report gen-drift` detect hand-edits to generated files; `drift` compares inlined blocks against the plugin tree; `extract-items` + `items` reconcile a module's `items[]` against what tree-sitter finds in the source; `gate` runs the profile's declared compile gates and records the result. |

Run `specdev-analyzer --help` for the full list; every subcommand has its own `--help`.

Today it targets **5 languages** (`rust`, `c`, `cpp`, `dart`, `typescript` — not all have a manifest
emitter), and the plugin it pairs with ships **7 profiles**, **16 blocks** and **6 bridges**.

Errors carry a structured `<path>:<line>:<col>: <message>` payload via `LocatedError`, so editors and
CI can jump to the failing site. Findings carry stable rule codes (`LAYOUT-00n`, `SCHEMA`,
`AGGREGATOR-00n`, `METASCHEMA-00n`, `BOOTSTRAP-00n`, `INLINE-00n`, `MANIFEST-00n`, …).

### Compatibility

The version number is not the compatibility check — `contract_version` is. Each profile manifest
declares one, and the analyzer refuses a manifest whose `contract_version` it does not implement
(`MANIFEST-002`), naming the file. Upgrade the analyzer when that fires.

## Install

Both repositories are **private**, so the install URL depends on which organization you were granted
access to. Read the org off the plugin's own remote rather than guessing it:

```sh
git -C "${CLAUDE_PLUGIN_ROOT}" remote get-url origin
# git@github.com:<org>/specdev.git  ->  <org>
```

Then:

```sh
cargo install --git https://github.com/<org>/specdev-analyzer
```

That URL is HTTPS while every remote in this project is SSH, so `cargo` needs a working HTTPS
credential for a private repo (a credential helper, or a `~/.gitconfig` `insteadOf` rule pointing
HTTPS at SSH). If you have repo access and the command still fails to authenticate, that mismatch is
why.

Requires a stable Rust toolchain. MSRV is **1.86**, set by the dependency graph rather than by this
crate's own source: `clap_builder` needs edition 2024, and eleven `icu_*` packages need 1.86.
Measured with `cargo check --lib` — 1.81 and 1.85 fail, 1.86 and 1.88 pass. CI declares an `msrv`
job that builds on exactly that toolchain, though note that workflow needs a cross-repo token it
does not yet have, so treat the number as measured-by-hand until it has a green run behind it.

Re-run `cargo install` with `--force` whenever the plugin updates. The two move together, and
nothing warns you when the installed binary falls behind — a stale binary can silently lack guards
the current plugin prose assumes.

## Usage

```
specdev-analyzer validate      [--project <dir>] [--strict] [--no-color]
specdev-analyzer extract       --output <dir> [--project <dir>]
specdev-analyzer extract-items [--project <dir>] [--module <Module>]
specdev-analyzer items diff    <Module> [--project <dir>]
specdev-analyzer reduce        [--project <dir>] [--output <path>]
specdev-analyzer resolve       [--project <dir>]
specdev-analyzer report        gen-plan | gen-drift | coverage [--project <dir>]
```

`--help` is authoritative; the list above is the common path, not the whole surface. There is no
`audit` subcommand — source-vs-spec comparison is `extract-items` plus the three-stage `items`
flow (`diff` → `propose` → `apply`).

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
├── src/                                        # source code (items-sync / reverse target)
└── specs/
    ├── modules/*.toml                          # user-authored module specs
    ├── architecture.toml                       # cross-module wiring
    ├── requirements.toml                       # requirements
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

Multi-package projects come in two shapes.

**Independent packages** own their own `spec.toml` and `specs/specdev/` tree and know nothing about
each other; run the analyzer once per package, from that package's directory.

**A declared workspace** (the `rust-workspace` profile) puts a `[workspace]` table with
`[[workspace.members]]` in the root `spec.toml`. `manifest --write` then renders a workspace-root
`Cargo.toml` (`[workspace]` + `[workspace.package]` + `[workspace.dependencies]`), and each member
emits a member manifest that inherits from it (`version.workspace = true`, and so on). Members still
carry their own `specs/specdev/` tree and their own profile.

## Library API

The crate exports a library surface for tools that want to drive the same plumbing rather than shell
out to the binary. See `src/lib.rs` for the entry-point map; `bootstrap`, `gen_lineage`, `derive`,
`inline` and the manifest renderers are the modules most worth reaching for. The binary in
`src/cli.rs` is a thin argument-parsing layer over it, so anything the CLI does is reachable from
the library.

## License

MIT. See `LICENSE.md`.
