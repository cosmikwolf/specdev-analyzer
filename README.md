# specdev-analyzer

Domain-agnostic Rust CLI that reads TOML specs, validates them against JSON Schema, extracts canonical YAML for codegen, and audits source code for drift against the specs.

Successor to [`sysml-v2-analyzer`](../sysml-v2-analyzer/). Pairs with the [`specdev`](../specdev/) Claude plugin.

## Status

Phase 1 — scaffolding. The CLI parses its arguments and prints usage, but the pipelines are not implemented yet. Progress is tracked in [`../specdev-next/00-status.md`](../specdev-next/00-status.md).

## Install

```
cargo install --git https://github.com/tenkai/specdev-analyzer
```

## Commands

```
specdev-analyzer validate <paths…>
specdev-analyzer extract -o <dir> <paths…>
specdev-analyzer audit --source <dir> --extracted <dir>
specdev-analyzer resolve --project <dir>
```

Each command prints a placeholder message until its phase lands:

| Command  | Lands in |
|----------|----------|
| validate | Phase 4  |
| extract  | Phase 5  |
| resolve  | Phase 7  |
| audit    | Phase 8  |

## License

MIT.
