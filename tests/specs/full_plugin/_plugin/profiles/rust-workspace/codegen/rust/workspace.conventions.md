# Workspace Cargo.toml conventions

Authoritative spec → `Cargo.toml` mapping for `rust-workspace` projects.
The workspace root's `Cargo.toml` is **OUTPUT** of `specdev-analyzer
cargo-toml`; never hand-edit it. Member crates' `Cargo.toml` files are
also OUTPUT of `cargo-toml` invoked at the member's spec.toml; the
analyzer detects the workspace ancestor and rewrites inheritable fields
to `*.workspace = true`.

## Workspace Cargo.toml shape

The workspace root emits in this order:

```toml
[workspace]
resolver = "..."          # spec.toml [workspace].resolver, default "2"
members = [
    "crates/<name-1>",    # spec.toml [[workspace.members]] entries,
    "crates/<name-2>",    # in declaration order
]

[workspace.package]
version = "..."           # architecture.toml [workspace_package] fields,
edition = "..."           # in canonical order:
rust-version = "..."      #   version → edition → rust-version → license → repository.
license = "..."           # Only present fields are emitted.
repository = "..."

[workspace.dependencies]
<crate> = "..."            # architecture.toml [[workspace_dependencies]],
<crate> = { version = "...", path = "...", default-features = false }
                           # lex-sorted by crate name; rendered identically
                           # to a [dependencies] entry.
```

## Member Cargo.toml shape (workspace-aware)

When `cargo-toml` is invoked at a member crate's spec.toml inside a
detected workspace:

* `[package]` fields whose canonical value lives in the workspace's
  `[workspace_package]` are emitted as `<field>.workspace = true`
  instead of literal copies. The five inheritable fields are:
  `version`, `edition`, `rust-version`, `license`, `repository`.
  `name` is always per-crate; `description` is always per-crate.

* `[dependencies]` entries whose member-spec `CrateReference.source =
  "workspace"` are emitted as `<dep> = { workspace = true, ... }`,
  carrying any per-member overlay (`optional = true`, additional
  `features = [...]`, `default-features = false`).

## Determinism

Every emission is byte-stable across runs. Members and dependencies are
emitted in their authoring order (workspace members) or lex-sorted
(workspace_dependencies). No timestamps, no machine-specific paths.
