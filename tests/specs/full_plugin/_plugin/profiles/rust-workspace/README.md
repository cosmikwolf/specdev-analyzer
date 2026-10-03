# `rust-workspace` profile

A specdev profile for Cargo workspace roots. It lists the members specdev coordinates and holds the workspace-tier
decisions and open questions; it does NOT own per-member structure (modules, requirements) — those live in each
member's own `specs/` tree. The root `Cargo.toml` is the project's: `[workspace]`, `[workspace.package]`,
`[workspace.dependencies]`, `default-members`, `exclude` and the build profiles live there.

## When to use

Use `rust-workspace` when your project is a multi-crate Cargo workspace. For a single-crate library, use
`library-api` directly. For a single-crate binary firmware project, use `firmware-layered`.

## Bootstrap flow

```bash
# Workspace root (run at the root):
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

# Then list the member in the root spec.toml:
#   [[workspace.members]]
#   name = "<member-name>"
#   path = "crates/<member-name>"
# (/spec-add-member does both steps.)
```

Every member listed in `spec.toml` is a member in the root `Cargo.toml`. A crate Cargo builds but specdev does not
coordinate (a tool, a bench) is not listed in the spec.

## Adopting an existing single-crate project as a member

You have a *validated* single-crate project (`library-api`,
`firmware-layered`, or `audio-plugin`) and want to make it the first
member of a new workspace. There is no skill for this yet; the move is
manual and **ordering-sensitive** — `bootstrap` refuses a target whose
`specs/specdev/` already exists (`BOOTSTRAP-001`), so the single-crate
baseline must leave the root *before* you bootstrap the workspace shell
there.

The move is clean because the tree is already partitioned the right
way: the id-bearing baseline (`spec.toml`, `specs/requirements/`,
`specs/decisions/`, `specs/open_questions/`, `specs/architecture.toml`,
`specs/modules/`) and the member's own machinery (`specs/specdev/`) all
relocate together, and every path
resolves relative to the member's `spec.toml` — so a moved member
validates standalone (bidirectional minimality, above). Move the whole
tree; do not bootstrap the member fresh and clobber it.

```bash
# From the existing single-crate project root:

# 1. Move the ENTIRE spec tree + marker into a member slot. `git mv` is a
#    byte-preserving rename, so requirement / decision / open-question ids
#    come along verbatim. This also frees the root of specs/specdev/,
#    which step 2 requires.
mkdir -p crates/<member-name>
git mv spec.toml crates/<member-name>/spec.toml
git mv specs     crates/<member-name>/specs
#    (repeat for any other pre-existing crate you're folding in)

# 2. The root is now clear. Bootstrap the workspace shell here:
specdev-analyzer bootstrap \
  --profile rust-workspace --language rust --name <project-name> .

# 3. Wire the member into the workspace (same as a fresh member):
#    - append [[workspace.members]] { name, path = "crates/<member-name>" }
#      to the root spec.toml
#    - move shared package fields and dependencies into the root
#      Cargo.toml's [workspace.package] / [workspace.dependencies]

# 4. Validate the member tree and the root:
specdev-analyzer check --project crates/<member-name>
specdev-analyzer check --project .
```

**Ordering trap:** if step 2 runs while the root still holds a
single-crate `specs/specdev/`, it fails with `BOOTSTRAP-001`. Move the
baseline out (step 1) first. If the adopted crate is on an older spec
format, run `specdev-analyzer migrate` at the root once the member is
listed — a workspace moves to a new format as one. `/spec-add-member`
automates this move.

## Composition with member profiles

The workspace profile carries no module / requirement surface — bidirectional minimality. Each member is a complete
spec on its own; extracting a member's tree and bootstrapping it standalone must produce a valid crate. A member
reaches another member as a Cargo path or workspace dependency, never through `depends_on`.

## Decisions and open questions at the workspace tier

The root holds `specs/decisions/` and `specs/open_questions/` (and, rarely, `specs/requirements/`). Workspace
decisions — the MSRV policy, an error-type convention across members — reach every member: `specdev-analyzer report
decisions --module <M>` in a member lists them. Member specs reference only their own decision ids, never workspace
ids, so each member stays self-contained.

Workspace open questions reach down the same way. At this tier `affects` is optional: a workspace question with no
`affects` reaches every module in every member, so `check` in each member reports it against every module with code
(`implementation_unblocked`) until it is resolved or a default is committed. Scope a workspace question narrower by
listing member requirement ids in `affects`. Use `/spec-question` and `/spec-decide`.

## Graph checks shipped

- `unique_member_names` — workspace member names are unique
- `member_paths_unique` — workspace member paths are unique
- `decision_ids_unique` — decision ids are unique
- `decision_supersedes_acyclic` — supersession chains have no cycle
- `decision_supersedes_chain_consistency` — every superseded decision is named by exactly one accepted successor
- `open_question_closed_by_resolves` — `closed_by` names an existing decision
- `open_question_status_consistency` — `closed_by` / `default_via` name accepted decisions

## Gates and CI

`specs/specdev/profile.yaml` declares the compile gates; the analyzer renders them into `scripts/specdev-gates.sh`,
which runs `specdev-analyzer check` first and then each gate. CI runs the script. Bootstrap writes
`.github/workflows/specdev.yml` once (yours afterwards); it reads the repository variable `SPECDEV_ANALYZER_REPO` and
the secret `SPECDEV_ANALYZER_TOKEN` to build the private analyzer.

**Toolchain CI installs:** the stable Rust toolchain (cargo), which also builds `specdev-analyzer`, plus whatever each member's gates need (a member's build target, a member's system libraries). Each member's own workflow file is not read by GitHub; add its toolchain steps to the root's.

CI installs are frozen: commit the lockfile, or the CI install refuses to change it.

The root's gate script runs each member's own script in declared order and stops at the first failure; the root
declares no gate of its own. The workspace moves to a new spec format as one: run `migrate` at the root, never in a
member.

## Layout under `specdev/profiles/rust-workspace/`

```
profile.yaml                  # dispatch: paired files (the CI workflow); no gates
spec.toml.template            # workspace marker + members list
domain.toml.template          # member-name and member-path uniqueness, ledger checks
guidance.md                   # implementation guidance (inlined)
schemas/
  architecture.schema.yaml    # the workspace architecture.toml (project decisions)
reductions/
  project.yaml                # Member entities + ledger kinds
templates/
  specdev-ci.yml.template     # the CI workflow, written once
README.md                     # this file
```

No `module.schema.yaml` — workspaces have no module surface; that's per-member.

## Why bootstrap drops many files into your tree

Each `specdev-analyzer bootstrap` inlines a complete copy of the active profile's schemas, reductions, guidance and
language tier under `specs/specdev/`, and every member gets its own self-contained copy. That is not duplication, it
is the "project owns its content" invariant: each member must be extractable and runnable as a standalone project.
With N members, expect N+1 inlined trees.
