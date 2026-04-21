# specdev-analyzer

Domain-agnostic CLI that turns TOML specs into first-class engineering artifacts: schema-validated, graph-checked, extractable to codegen-ready YAML, and auditable against real source code.

Pairs with the [`specdev`](https://github.com/tenkai/specdev) Claude Code plugin, but works standalone — `cargo install`-able and has no runtime dependency on the plugin.

## What it does

- **`validate`** — parses every spec file under `specs/`, checks it against its JSON Schema (authored in YAML, compiled at startup), and runs the graph-level invariants the active domain declares (layer ordering, reachability from the initial state of a state machine, port wiring, resource ownership, acyclic module dependencies, …). Errors carry stable rule ids (`LAYER001`, `FSM020`, `PORT030`, …) so skills and CI can map them to fixes.
- **`extract`** — emits canonical, byte-stable YAML for codegen: one file per module, plus a synthesized `architecture.yaml` (module summaries + dependency graph + raw connections) and a flattened `requirements.yaml`. All keys alphabetized at every level so diffs are meaningful.
- **`audit`** — parses the project's Rust / C / C++ source with tree-sitter and cross-checks it against the extracted spec, reporting Match / Missing / Mismatch / Uncovered per module. Use it to catch drift as code evolves faster than the spec.
- **`resolve`** — prints the effective domain, language, schema, and codegen paths for a project, respecting the `spec.toml` → `.specdev/` → plugin defaults override chain. Skills embed its output as their context header.

## Install

```
cargo install --git https://github.com/tenkai/specdev-analyzer
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
$ specdev-analyzer validate --project ./my-firmware
✓ 4 modules validated, 6 requirements, 3 connections
  no findings

$ specdev-analyzer extract --project ./my-firmware --output ./gen
wrote gen/AudioPipeline.yaml
wrote gen/BtA2dpSink.yaml
wrote gen/I2sOutput.yaml
wrote gen/StatusLed.yaml
wrote gen/architecture.yaml
wrote gen/requirements.yaml
```

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
