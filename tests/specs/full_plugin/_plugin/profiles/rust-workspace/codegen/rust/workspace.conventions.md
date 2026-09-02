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
default-members = [       # spec.toml [workspace].default_members — member
    "crates/<name-1>",    # NAMES in the spec, each resolved to that member's
]                         # path; declaration order; omitted when empty
exclude = [               # spec.toml [workspace].exclude — paths, verbatim,
    "tools/<dir>",        # declaration order; omitted when empty
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

[profile.<name>]           # architecture.toml [profiles.<name>] — dev, release,
opt-level = ...            # test, bench in that order, then custom profiles
debug = ...                # lex-sorted; keys in a fixed order; omitted entirely
                           # when the spec declares no profiles
[profile.<name>.package.<crate>]
opt-level = ...            # right after its parent profile, crates lex-sorted
```

## Build profiles are root-only

`[profile.*]` tables come from the workspace root's `architecture.toml`
`[profiles.<name>]` and are emitted only into the root `Cargo.toml`.
Cargo ignores profiles declared in a member manifest, so a member
architecture that declares `profiles` fails `validate`
(`PROFILES-ROOT-ONLY`, naming the root) and the member-mode render
refuses them with a warning and emits nothing. The subset is closed:
`opt-level`, `debug`, `lto`, `codegen-units`, `panic`, `strip`,
`overflow-checks`, `debug-assertions`, `incremental`, and
`package.<crate>` overrides of the seven Cargo allows there (no `panic`,
`lto`, `rpath`). A custom profile carries `inherits`. Values keep
Cargo's spelling: `opt-level = "s"`, `debug = 2`, `lto = "fat"`.

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

Every emission is byte-stable across runs. Members, default members and
excludes are emitted in their authoring order; dependencies are
lex-sorted (workspace_dependencies). No timestamps, no machine-specific
paths.

A `default_members` name that matches no `[[workspace.members]]` entry
is an error at emit time as well as at validate time — `manifest` does
not run `validate`, and Cargo rejects a `default-members` path that is
not a member.
