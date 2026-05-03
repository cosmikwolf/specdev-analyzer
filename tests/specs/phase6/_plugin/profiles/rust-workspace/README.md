# `rust-workspace` profile

A specdev profile for Cargo workspace shells. Owns the workspace root's
`Cargo.toml` (`[workspace]`, `[workspace.package]`,
`[workspace.dependencies]`); does NOT own per-member structure (modules,
requirements, features) — those live in each member's own `specs/` tree.

## When to use

Use `rust-workspace` when your project is a multi-crate Cargo workspace
where two or more members share `[package]` metadata or dependencies.
For a single-crate library, use `library-api` directly. For a
single-crate binary firmware project, use `firmware-layered`.

## Bootstrap flow

```bash
# Workspace shell (run at the workspace root):
specdev-analyzer bootstrap \
  --profile rust-workspace \
  --language rust \
  --name <project-name> \
  .

# Per member (run inside each member's directory):
mkdir -p crates/<member-name> && cd crates/<member-name>
specdev-analyzer bootstrap \
  --profile library-api \
  --language rust \
  --name <member-name> \
  .

# Then add the member to the workspace:
#   1. Edit the workspace's spec.toml: append [[workspace.members]]
#      with name = "<member-name>", path = "crates/<member-name>"
#   2. Edit the workspace's specs/architecture.toml: declare any
#      hoisted candidates under [[workspace_dependencies]] and any
#      shared package fields under [workspace_package]
```

## Composition with member profiles

The workspace profile carries no module / requirement / feature surface
(D36 — bidirectional minimality). Each member is a complete spec on its
own; extracting a member's `specs/specdev/` and bootstrapping it as a
standalone project must produce a valid library (or firmware) crate.

A member opts into workspace inheritance at two layers:

1. **Package fields:** the member omits `[project].version /
   .license / .repository` etc.; the cargo-toml emitter walks up to
   the workspace's `[workspace_package]` and emits
   `<field>.workspace = true`.
2. **Dependencies:** the member sets `source = "workspace"` on a
   `CrateReference`; the cargo-toml emitter renders `<dep> =
   { workspace = true, ... }`.

Both opt-ins are per-field / per-dep — a member can inherit `edition`
from the workspace while keeping its own `version`, or inherit a
registry crate from `[workspace.dependencies]` while keeping a
local-only dep inline.

## Layout under `specdev/profiles/rust-workspace/`

```
spec.toml.template            # workspace marker + members list
architecture.toml.template    # [workspace_package] + [[workspace_dependencies]]
domain.toml.template          # member-name uniqueness + path uniqueness
requirements.toml.template    # rare workspace-level requirements
schemas/
  workspace.schema.yaml       # validates spec.toml's [workspace] table
  architecture.schema.yaml    # validates the workspace architecture.toml
reductions/
  project.yaml                # Member + WorkspaceDependency entities
codegen/
  rust/
    workspace.conventions.md  # cargo-toml emission rules
README.md                     # this file
```

No `module.schema.yaml` / `module.conventions.md` — workspaces have no
module surface; that's per-member.
