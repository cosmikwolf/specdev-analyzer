# specdev-analyzer

Domain-agnostic CLI that turns TOML specs into first-class engineering artifacts: schema-validated, graph-checked, extractable to codegen-ready YAML, and auditable against real source code.

Pairs with the [`specdev`](https://github.com/cosmikwolf/specdev) Claude Code plugin, but works standalone — `cargo install`-able and has no runtime dependency on the plugin.

## What it does

- **`validate`** — parses every spec file under `specs/`, checks it against its JSON Schema (authored in YAML, compiled at startup), and runs the graph-level invariants the active domain declares (layer ordering, reachability from the initial state of a state machine, port wiring, resource ownership, acyclic module dependencies, …). Errors carry stable rule ids (`LAYER001`, `FSM020`, `PORT030`, …) so skills and CI can map them to fixes.
- **`extract`** — emits canonical, byte-stable YAML for codegen: one file per module, plus a synthesized `architecture.yaml` (module summaries + dependency graph + raw connections) and a flattened `requirements.yaml`. All keys alphabetized at every level so diffs are meaningful.
- **`audit`** — parses the project's Rust / C / C++ source with tree-sitter and cross-checks it against the extracted spec, reporting Match / Missing / Mismatch / Uncovered per module. Use it to catch drift as code evolves faster than the spec.
- **`resolve`** — prints the effective domain, language, schema, and codegen paths for a project, respecting the `spec.toml` → `.specdev/` → plugin defaults override chain. Skills embed its output as their context header.

## Install

```
cargo install --git https://github.com/cosmikwolf/specdev-analyzer
```

Requires a stable Rust toolchain.

## Usage

```
specdev-analyzer validate [--project <dir>] [--domain <domain.toml>] [--no-color]
specdev-analyzer extract  --output <dir> [--project <dir>] [--domain <domain.toml>]
specdev-analyzer audit    [--project <dir>] [--filter <Module>] [--show-uncovered] [--expand]
specdev-analyzer resolve  [--project <dir>] [--plugin-root <dir>] [--allow-missing]
```

`--project` defaults to the current directory. The project root is the directory containing `spec.toml`.

### Example

```
$ specdev-analyzer validate --project ./blinky-a2dp
0 error(s), 0 warning(s), 0 info(s)

$ specdev-analyzer extract --project ./blinky-a2dp --output ./gen
wrote 6 file(s) to ./gen
```

`extract` writes one YAML per module plus `architecture.yaml` and `requirements.yaml`.

### Audit

`audit` compares the extracted spec to the project's source tree. Run it after a
`spec-gen` pass (or against a hand-written source tree) to see drift:

```
$ specdev-analyzer audit --project ./blinky-a2dp --show-uncovered
AudioPipeline (not found):
  + file AudioPipeline — no source file found for module `AudioPipeline`

BtA2dpSink (not found):
  + file BtA2dpSink — no source file found for module `BtA2dpSink`

…

0 match, 4 missing, 0 mismatch, 0 uncovered
```

A fully-generated project produces a `match` line per module and zero `missing`.

### Resolve

`resolve` is primarily for skills: it prints the effective domain, language, and
codegen paths after applying the `spec.toml` → `.specdev/` → plugin-defaults
cascade, so a skill's context header always points at the right files.

```
$ specdev-analyzer resolve --project ./blinky-a2dp
## Spec Context

- Project root: `/.../blinky-a2dp`
- Domain: `firmware`
- Language: `rust`
- Codegen name: `firmware-rust`

## Resolved Paths

- Schema:   `.../schema/firmware/v1`          _(from plugin default)_
- Domains:  `.../domains/firmware`            _(from plugin default)_
- Languages:`.../languages/rust`              _(from plugin default)_
- Codegen:  `.../codegen/firmware-rust`       _(from plugin default)_
```

Each line's provenance tag shifts to `from spec.toml` or `from .specdev/` once a
project starts overriding.

### Validation errors

Errors carry a stable rule id and a structured path pointer. Example —
`AudioPipeline` (middleware) declared a dependency on `StatusLed` (also
middleware), which the firmware layer rules disallow:

```
$ specdev-analyzer validate --project ./blinky-broken
error: [LAYER001] project/modules/0/depends_on/2: module `AudioPipeline` (layer middleware) depends on `StatusLed` (layer middleware), not in allowed_deps ["driver", "HAL", "PAC"]
1 error(s), 0 warning(s), 0 info(s)
```

Rule ids (`SCHEMA`, `LAYER001`, `FSM020`, `PORT030`, `OWN050`, `ISR060`, …) are
stable across versions; skills and CI rely on them to suggest specific fixes.

## Project layout

A single-crate project looks like:

```
my-firmware/
├── spec.toml                  # project config (domain, language, spec_version, paths)
├── src/                       # source code
└── specs/
    ├── architecture.toml      # cross-module wiring (connections, mounts, navigation)
    ├── modules/*.toml         # one module per file
    └── requirements/*.toml    # requirements referenced by modules
```

A multi-crate workspace declares `[[packages]]` in `spec.toml` and nests module specs one level deeper:

```
my-workspace/
├── spec.toml                  # [[packages]] name + source_path per crate
├── crates/core/     src/…
├── crates/drivers/  src/…
└── specs/
    ├── architecture.toml      # endpoints may be <pkg>::<Module>.<port>
    ├── modules/
    │   ├── core/Clock.toml
    │   └── drivers/Led.toml   # depends_on = ["core::Clock"]
    └── requirements/*.toml    # requirements are not packaged
```

The `package` for each module is derived from the filesystem path, never written inside the TOML. Cross-package references use `<pkg>::<Module>` syntax; unqualified names resolve within the same package first, then globally (ambiguous resolution is a hard error).

## Domains, languages, and overrides

The analyzer core is **domain-agnostic**: it loads schemas, domain rules, and language adapters from external directories rather than hard-coding them. The paired `specdev` plugin ships two domains (`firmware`, `egui`) and three languages (`rust`, `c`, `cpp`); you can add more without touching this crate.

Resolution order for each kind of artifact (schema / domain / language / codegen):

1. Explicit path in `spec.toml` `[paths.overrides]`
2. `<project>/.specdev/<kind>/<name>/`
3. Plugin defaults shipped alongside the plugin

This lets you fork a domain locally, prototype a new schema rule, or extend a language adapter without modifying upstream.

## Checks at a glance

Every domain declares its graph checks in `domain.toml` as thin wrappers over five declarative primitives:

| Primitive         | Use case                                                     |
|-------------------|--------------------------------------------------------------|
| `unique`          | No two modules own the same pin; no duplicate requirement id |
| `reference_exists`| `satisfies` ids resolve; connection endpoints are declared   |
| `reachability`    | Every FSM state reachable from the initial state             |
| `acyclic`         | Module dependencies form a DAG                               |
| `layer_dependency`| Dependencies flow downward only (HAL → PAC, never upward)    |

Rule ids are stable across versions so CI and editor integrations can rely on them.

## License

MIT. See `LICENSE.md`.
