# `specdev-analyzer` integration tests

## Naming convention

Test files, modules, functions, fixture directories, and temp-dir prefixes are
named **for the feature they exercise**, never for when they were authored.
There is no chronological ("phaseN", "stepN") nomenclature anywhere in this
tree. Planning-phase terminology and decision IDs live in the planning
workspace, not in shipped surface — a reader of this repository cannot follow a
`Phase 16 §8.1 step 3a` or `(D51)` pointer, so those references are removed.

Rules:

- **File name** = the feature under test (`cargo_toml_shape_dispatch.rs`,
  `egui_nih_plug_bridge.rs`), discovered automatically by `cargo test`.
- **Helper fns** that return a fixture path are named for the fixture
  (`full_plugin()`, `minimal_plugin()`), not for a phase.
- **Temp-dir prefixes** use the feature slug (`specdev-cargo-toml-shape-{tag}-…`).
- **Error-code families** (`BRIDGE-LEX-001`, `ITEMS-001`, `TEST-KIND-001`, …) and
  **dogfood finding IDs** (`F-006`) are domain identifiers, not planning
  pointers — they stay.

A CI lint (`.github/scripts/agnosticism-lint.sh`) enforces the absence of phase
nomenclature and decision-ID references across `src/` and `tests/`. Use a
trailing `// PLANNING-OK: <reason>` to escape a line that genuinely needs such a
token (none currently do).

## Shared fixture trees (`tests/specs/`)

| Directory | Contents |
|---|---|
| `full_plugin/_plugin/` | The complete canonical plugin tree (every profile, block, bridge). Byte-for-byte mirror of the live `specdev/` plugin; consumed by most tests via the `full_plugin()` helper. |
| `minimal_plugin/_plugin/` | A small early plugin tree (embassy + lvgl + firmware-layered). Consumed via `minimal_plugin()`. |
| `dual_profile_plugin/_plugin/` | A plugin tree carrying two profiles (firmware-layered + library-api) plus the rust language tier. |
| `reduction_cases/` | Loose reduction / extract fixtures (unresolved-extends, bridge lex-order violation, extract-end-to-end, …). |
| `validate_cases/` | Loose validate-pipeline / layout-discovery fixtures (malformed `spec.toml`, aggregator duplicate-id, minimal skeleton, …). |

## Test-file map

Feature-scoped names for the integration suites:

| File | Exercises |
|---|---|
| `validate_pipeline.rs` | Validate pipeline against the `specs/specdev/` layout; file:line-precision errors. |
| `reduction_and_extract.rs` | Reduction resolve + extract pipeline. |
| `bootstrap_and_inline.rs` | Bootstrap golden path, block/bridge inline, lineage stability, lex-order fail-fast. |
| `merge_and_drift.rs` | `--merge` and `drift`. |
| `profiles_end_to_end.rs` | First profiles end-to-end. |
| `embassy_stm32_blocks.rs` | First concrete blocks (embassy + stm32h5-rs). |
| `plugin_lint_blocks.rs` | `plugin-lint blocks` extension. |
| `crate_reference_picks.rs` | Embedded `CrateReference` + per-profile project-wide picks. |
| `bridges_end_to_end.rs` | First canonical bridge (`embassy+stm32h5-rs`) + project-local bridge. |
| `prompt_assembly.rs` | Deterministic prompt assembly. |
| `cargo_toml_emission.rs` | Deterministic `Cargo.toml` emission. |
| `crate_reference_source.rs` | Closed-set `CrateReference.source` (F-004). |
| `rust_workspace_profile.rs` | `rust-workspace` profile + workspace `cargo-toml` emission. |
| `orphan_requirements.rs` | Orphan-requirements check + `nature` field (F-006). |
| `requirement_id_relaxation.rs` | `RequirementId` pattern relaxation + early quick-wins (F-002/3/5). |
| `items_extract_and_reverse.rs` | Items extraction surface + reverse pre-flight + `verified_by_resolves`. |
| `context_subcommand.rs` | `context --module <Name>` cone projection + YAML emission. |
| `context_files.rs` | `module.context_files` schema field + prompt inlining. |
| `language_tier.rs` | `languages/<lang>/` plugin tier through layout + bootstrap + aggregator. |
| `module_items_field.rs` | Profile `module.schema.yaml` `items[]` field + bootstrap substitution. |
| `items_diff.rs` | `items diff` deterministic detection. |
| `items_propose.rs` | `items propose` deterministic scaffolding. |
| `items_apply.rs` | `items apply` mutation + approval gate. |
| `audio_plugin_profile.rs` | `audio-plugin` profile. |
| `nih_plug_block.rs` | `nih-plug` block. |
| `egui_block.rs` | `egui` block. |
| `egui_nih_plug_bridge.rs` | `egui+nih-plug` bridge. |
| `hello_plugin_smoke.rs` | hello-plugin full-stack smoke fixture. |
| `agnosticism_bugfixes.rs` | Bug + UX fixes from the audio-plugin dogfood. |
| `capability_tagging.rs` | Capability tagging, `--for` filter, `ITEMS-001`/`CONTEXT-100` checks. |
| `manifest_loaders.rs` | Embedded schemas + manifest loaders. |
| `manifest_authoring.rs` | Load every authored manifest in the plugin tree + fixture mirror. |
| `manifest_copy_on_bootstrap.rs` | Bootstrap-time `profile.yaml` copy + inline-time `deps.yaml`/`contract.yaml` copy with content_hash. |
| `cargo_toml_shape_dispatch.rs` | `cargo_toml.rs` shape-driven dispatch off `profile.yaml`. |
| `cargo_toml_bin_gate.rs` | `cargo_toml.bin.gate` interpreter. |
| `cargo_toml_paired_stub.rs` | `cargo_toml.bin.paired_stub.template_file` read. |
| `block_deps_merge.rs` | Block `deps.yaml` consumption in `merge_dep`. |
| `profile_layout_mismatch.rs` | `PROFILE-LAYOUT-MISMATCH` validate-time check. |
| `context_profile_specific.rs` | `context`'s manifest-driven `profile_specific` field. |
| `bootstrap_workspace_steps.rs` | `bootstrap.rs` workspace-step dispatch off `profile.yaml`. |
| `bridge_co_residency_hint.rs` | `reduction.rs::bridge_co_residency_hint` manifest-driven hint. |
| `validate_layout_dispatch.rs` | `validate_layout` surface-driven (manifest) dispatch. |
| `migrate_subcommand.rs` | `specdev-analyzer migrate` subcommand. |
| `agnosticism_lint.rs` | The agnosticism CI lint (`agnosticism-lint.sh`). |
| `bootstrap_name_substitution.rs` | Bootstrap-time `{{project_name}}` substitution coverage. |
| `validate_layout_suppression.rs` | `validate_layout` surface-driven suppression of `specs/modules/`. |
| `parameter_bindings.rs` | `[[parameter_bindings]]` (audio-plugin). |
| `test_items.rs` | Test items + test-aware validation. |

Helper suites keep descriptive names already: `coverage_report.rs`,
`gen_lineage.rs`, `hello_firmware.rs`, `triad_smoke.rs`.
