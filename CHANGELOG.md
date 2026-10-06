# Changelog

All notable changes to specdev-analyzer are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This file starts at the first tagged release; earlier versions were never
tagged, so there is no fixed commit to anchor a historical entry to.

The analyzer and the `specdev` plugin version **independently**. They are not
meant to match. `contract_version` is the compatibility number between them,
and it is `1`.

## [Unreleased]

### Fixed

- **Tests made by a local `macro_rules!` macro are discovered** (#223). A macro defined in the file whose rule body
  holds the test attribute makes a test per call, named by the call's first identifier after its attributes; an
  `ignore` attribute in the call makes it pending. Generic captures `@test_macro` / `@test.macro`; when several
  patterns name one test, the earliest name wins.
- **An import inside test-only code is not a dependency** (#224). `@import.test_scope` marks such code;
  `internal_edges_declared` skips the imports inside it, and `report imports` shows them with `test_only: true`.
- **An ambiguous `verified_by` name names its candidates** (#225): "more than one test; name one of: …" instead of
  "no discovered test".

## [0.3.0] — 2026-10-05

**Checked intent: the analyzer no longer generates code; `check` holds the spec against the code.** Pairs with
specdev 0.3.0. Profile `contract_version` 2; project format `spec_version` 2.0.

### Removed

- The code-generation subcommands and their state: `gen`, `prompt`, `gen-record`, `derive`, `manifest`,
  `cargo-toml`, `extract`, `report gen-drift`, `report gen-plan`, `gen-lineage.toml`, `deps.yaml` readers and the
  reference-pin and reference-crate lints.

### Added

- **`check`:** `validate`, then the checks that read the code — the source rule, stale `planned` flags, requirement ↔
  test on a test-discovery pass (`verified_by_resolves`, `requirements_have_tests`, `stale_test_pending`), internal
  imports against `depends_on`, the open-question gate (it keys on code existing), and each inlined rule pack's code
  checks; `--since <ref>` reports every new flag, `hard_gate` flip and default committed without a person.
- **Rule packs:** `checks.toml` per block or bridge, loaded at `validate` / `check`; primitives `unique` with
  `fields` and `related_attrs_allowed`; spec ↔ code checks with a query and an attribute mapping
  (`pack_entity_in_code`, `pack_attribute_matches`, `pack_key_ambiguous`); `plugin-lint` checks packs.
- **`migrate` to 2.0:** splits the ledgers one file per entry, moves test items to `verified_by`, reports or
  (`--plan-unresolved`) marks modules with no source, rewrites profile files with a baseline, moves a workspace as one,
  and refuses before any write when it cannot. `bootstrap` writes 2.0.
- **`gate`:** runs a declared gate; `--emit` writes `scripts/specdev-gates.sh`. No results are stored.
- `report tests`; `report imports` now resolves each import to a project file; `context` lists the inlined guidance; `inline` and `migrate` carry guidance and
  rule packs.

### Changed

- Hashes are over LF bytes, so a Windows checkout no longer reads every file as edited (#219).
- An absent unit directory or `specs/modules/` is "none yet": a fresh clone of a project no longer warns `LAYOUT-003`.
- Help texts describe what the code does; the removed-terms lint covers the plugin tree and the CLI help.

## [0.2.1] — 2026-09-24

### Added

- **`report imports`** lists packages a module's source file imports but
  the module may not rely on: not in its own `external_deps`, the
  architecture-level `external_deps`, or the deps of the blocks and bridges
  it uses. Such an import resolves only because a sibling declares the
  package, so every gate passes until the sibling drops it (#207). Exit 1
  when any is found. TypeScript and Dart, through a new `imports.scm` query
  and `LanguageAdapter::import_package`; a language without the query reports
  `checked: false`, never clean. Runtime deps only; test files are not read.
- **`validate` says when the plugin is ahead** (`PLUGIN-AHEAD-001`, info): when
  a plugin root is named (`--plugin-root`, `SPECDEV_PLUGIN_ROOT`,
  `CLAUDE_PLUGIN_ROOT`) and it holds newer copies of files this project
  inlined or bootstrapped, one line counts them and names `/spec-drift`.
  Decided by content hash through `drift`'s classifier, so a release that
  changes nothing this project uses stays silent (#210).
- **Relations built from entity attributes** (`match:` in a reduction, in
  place of `from_spec` + `endpoints`): one edge from every `from_kind`
  entity whose attribute is in a list to every `to_kind` entity whose
  attribute is in another list. Lets a profile state an edge no spec field
  carries (#208).
- **`context` lists containment** in two new buckets, `contains` and
  `contained_by`, for Module → Module relations that extend canonical
  `contains` (#208). They are not dependencies, so `upstream` / `downstream`
  keep their meaning; they count in `related_files` and
  `excluded_module_count`.
- **`entity_count` graph-check primitive** (#208): the number of scoped
  entities with an attribute value must equal `expected_count`; a project with
  none of the scope yet passes.
- Every relation in the graph (`reduce` output) now carries `extends`, its
  canonical relation kind, as entities already did.
- **`report decisions`** shows which decision text reaches which module's
  codegen prompt. `modules`: per module, its decisions and open questions,
  why each is there (`guided_by`, a satisfied requirement's `decided_by`, the
  workspace tier, an open question's `affects`) and which prose fields go in
  (`codegen_include`). `decisions`: per decision, the modules it reaches —
  empty when no prompt carries it. Computed by the function the prompt is
  rendered from; the prompt text itself is byte-identical (checked on 170
  real module prompts). `--module` / `--decision` narrow either view.
- **`report graph`** (#203) renders the module dependency graph as Mermaid
  (default) or DOT. When `domain.toml` declares a `layer_dependency` graph
  check, nodes are grouped by that check's layer field in its declared
  order, and every edge the check flags is drawn thick and red — the same
  rule `validate` applies, so the two cannot disagree. Each node shows how
  many modules it uses and how many use it. Block entities nested inside a
  module file are left out. The output is deterministic, so a committed map
  shows a new edge in a diff. `--check <rule>` picks the layer check when
  there is more than one; `-o <file>` writes to a file.
- **A warning when the installed analyzer is older than the plugin** (#179).
  Every command reads `shared/min-analyzer-version` from the plugin root it
  was given (`--plugin-root`, else `SPECDEV_PLUGIN_ROOT`, else
  `CLAUDE_PLUGIN_ROOT`) and prints one `ANALYZER-VERSION-001` line on stderr
  when this binary is older; `--version` prints it too. It never changes an
  exit code or stdout, and it is silent when no plugin root is named or the
  plugin has no such file. Before this, a binary a major version behind ran
  for months with nothing to say so.

### Changed

- **A reduction relation with neither or both of `from_spec` and `match`, or
  a misshapen `match`, is `METASCHEMA-006`** (#208) — for profile reductions
  too, not only block reductions. Before, a relation with no `from_spec` built
  no edges and said nothing.
- **`derive` exits 3 when it holds a target back** (#160). A run whose report
  has an `edited` or `conflict` line used to exit 0, so
  `derive --write && next-step` in CI proceeded past a file it did not write.
  It now exits **3**; every other target is still written, and 0 still means
  nothing was held back. The preview (no `--write`) uses the same code.
  `specdev`'s `/spec-gen` treats 3 as "report and continue"; an older
  `/spec-gen` treats it as a refusal and stops before recording lineage, so
  update the plugin with the analyzer.
- **`gen` and `prompt` exit 2, not 1, when they cannot run** (#165). An
  unparsable module, an unreadable `context_files` entry or a missing plugin
  root used to exit 1 — the same code as the open-question refusal. Exit 1 is
  now only the two codegen refusals (an open question; no module-scope gate
  declared).
- **`codegen_include` is honoured** (#217). It was documented in every
  decisions schema and read by nothing; the prompt always carried `text` +
  `consequences`. It now selects the prose fields, in a fixed order. The
  default is byte-identical to the old output, so no recorded `input_hash`
  moves.

### Fixed

- **`gen-plan` finds a module's file at the convention path** (#216). It
  consulted only an explicit `source_file` before calling a module with no
  lineage row `new`, while `extract-items` resolved the same module by the
  language's convention path. In nextjs-app, where no module names a
  `source_file`, a committed file read `new` and `/spec-gen --all` would have
  overwritten it. It now reads `unrecorded`.
- **A module record retires the module's stale body rows** (#218). A row left
  behind kept the old `input_hash`, so the module read `stale` on every run
  however often it was regenerated. Derived rows stay.
- **`validate` no longer drops its whole report when a spec file does not
  parse** (#196). A module, `decisions.toml` or sibling file with a TOML
  syntax error stopped the run at exit 2 with only the raw parse message: no
  count line, and every other finding lost, including the `LAYOUT-005` for the
  same file. It is now a counted `REDUCE-001` error ("no graph check ran"),
  the rest of the report is printed, and the exit code is 1 like any error
  finding.
- **`--no-color` is accepted by every subcommand** (#123). It was declared on
  `validate` alone, so `drift --no-color`, `report gen-plan --no-color` and
  the rest exited 2 on an unknown argument. It is now a global flag; output
  without color is unchanged by it.
- Test fixtures mirror the plugin's `decision_ids_unique` check (#197) and its
  one-owner rule for `pnpm-workspace.yaml` (#214); no analyzer change was
  needed for either.

## [0.2.0] — 2026-09-15

First tagged release. Pairs with `specdev` **v0.2.1**.

### Fixed

- **The crate could not be installed the way the docs tell you to install it.**
  `cargo install --git <url>` failed with eight `couldn't read … .scm` errors,
  because the tree-sitter query files were included across the repository
  boundary from a sibling `specdev/` checkout that `cargo install` never
  creates. The queries are now embedded in the crate. If you tried to install
  this tool and it would not compile, this was why.
- **`manifest --write` no longer destroys a hand-authored build manifest.** It
  refuses (`MANIFEST-003`, exit 1) when the file on disk differs from what the
  spec renders **and** has no record of being generated by this tool. This bit
  hardest on `nextjs-app`, where `/spec-init` selects the profile *because* it
  found your `package.json`, and the first `/spec-gen` then replaced it. The
  gen-owned sibling `pnpm-workspace.yaml` is on the same guard, and every file
  a write would touch is checked **before** any is written — a refusal never
  leaves a half-written tree.
- **`bootstrap` no longer overwrites existing spec files.** `spec.toml`,
  `architecture.toml`, `decisions.toml`, `open_questions.toml` and
  `requirements.toml` are checked first; a collision refuses and names every
  conflicting file (`BOOTSTRAP-003`).
- **`gen-plan` no longer calls hand-authored source "new".** A module whose
  declared source exists but was never recorded as generated now reports
  `unrecorded` and requires confirmation, instead of silently planning to
  generate over it.
- **`items apply` no longer overwrites its own audit trail.** Each run writes a
  log stamped with its own timestamp, so a second apply keeps the first's
  record.
- **`report gen-drift` distinguishes "no data" from "clean".** The two used to
  look identical, so a project that had never generated anything reported as
  healthy.
- The TypeScript extractor now sees destructured exports — array and object
  patterns, including rename, default and rest bindings. A module written that
  way was previously invisible to `items diff` and could drift with no warning.

### Added

- `bootstrap --force`, to overwrite an existing spec tree deliberately.
- `[tooling].analyzer_version` is stamped into a project's `lineage.toml`, so a
  bug report can name the exact pair of tools that produced a tree.

### Changed

- The README was rewritten. It described a five-subcommand, early-stage tool;
  the CLI ships 24 subcommands across five languages. Stale claims were removed,
  including a documented `audit` subcommand that does not exist and a minimum
  Rust version eleven minor releases out of date. The real minimum is **1.86**,
  and a CI job builds on exactly that toolchain so the number stays tested.
- Install instructions no longer hardcode one organization; they work against
  whichever fork you were granted access to.

### Note on this release's testing

CI ran for the first time in this repository's history during this release
cycle: eight jobs across two operating systems, under `--no-fail-fast` so a
failure list is complete rather than truncated at the first failing binary.
Two defects were found that way and are fixed above.
