# rust-workspace — implementation guidance

Rules an agent follows when it edits the root of a `rust-workspace` project: a Cargo workspace whose members are
specdev packages of their own. No check verifies these rules; review does.

## The root

- The root `Cargo.toml` is the truth for the workspace: members, default members, excludes, shared package fields,
  shared dependencies and build profiles. The spec keeps only `[[workspace.members]]` — the members specdev
  coordinates.
- Every member listed in `spec.toml` is a member in `Cargo.toml`. A crate Cargo builds but specdev does not
  coordinate (a tool, a bench) is not listed in the spec.
- Build profiles live in the root manifest only; Cargo ignores them in a member manifest.

## Members

- Each member has its own `specs/specdev/`; no spec state is shared across members. A member reaches another member as
  a Cargo path or workspace dependency, never through `depends_on`.
- A member inherits the shared package fields (version, edition, rust-version, license, repository) and shared
  dependencies from the root with `workspace = true`; `name` and `description` stay per crate.
- Run per-member skills from the member's directory.

## Checks and format

- The root's gate script runs each member's own script in declared order and stops at the first failure.
- The workspace moves to a new spec format as one: run `migrate` at the root, never in a member.
