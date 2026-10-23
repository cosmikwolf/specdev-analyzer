//! The analyzer's integration tests, as one program.
//!
//! Each `tests/*.rs` file is a module here; a new test file needs a line
//! below. `requirement_id_relaxation.rs` is its own target (it changes the
//! process environment). Run one file with `cargo test --test suite <file>::`.

// Shared helpers, compiled once; a test file imports them with `use crate::ci;`.
#[path = "common/checked_intent.rs"]
mod ci;
#[path = "common/ledger.rs"]
mod ledger;

mod agnosticism_bugfixes;
mod agnosticism_lint;
mod analyzer_version;
mod attribute_argument_spike;
mod audio_plugin_profile;
mod bootstrap_and_inline;
mod bootstrap_collision_guard;
mod bootstrap_name_substitution;
mod bootstrap_templates;
mod bootstrap_workspace_steps;
mod bridge_co_residency_hint;
mod bridges_end_to_end;
mod check_plugin_ahead;
mod checked_intent_checks;
mod checked_intent_matrix;
mod checked_intent_migrate;
mod cli_help_text;
mod clone_layout;
mod code_root;
mod context_files;
mod context_profile_specific;
mod context_subcommand;
mod coverage_report;
mod crate_reference_picks;
mod crlf_hashes;
mod dart_adapter;
mod dart_items;
mod egui_block;
mod egui_nih_plug_bridge;
mod embassy_stm32_blocks;
mod embedded_language_queries;
mod field_cuts;
mod field_feedback_2;
mod field_feedback_3;
mod flutter_app_profile;
mod gate_registry_schema;
mod gate_run;
mod gate_script;
mod global_no_color;
mod hello_firmware;
mod hello_libraries;
mod hello_plugin_smoke;
mod hello_pyo3;
mod hello_python;
mod hello_rtic;
mod items_apply;
mod items_diff;
mod items_extract_and_reverse;
mod items_extraction_snapshot;
mod items_propose;
mod language_tier;
mod legacy_fold;
mod manifest_authoring;
mod manifest_copy_on_bootstrap;
mod manifest_loaders;
mod merge_and_drift;
mod migrate_review_fixes;
mod migrate_review_fixes_2;
mod migrate_subcommand;
mod module_items_field;
mod nih_plug_block;
mod orphan_requirements;
mod paired_files;
mod parallel_safe_layout;
mod parameter_bindings;
mod per_entry_layout;
mod plugin_ahead_notice;
mod plugin_lint;
mod plugin_lint_blocks;
mod profile_contract;
mod profile_substrate_drift;
mod profiles_end_to_end;
mod prompt_assembly;
mod python_tier;
mod reduction_and_extract;
mod render_containment;
mod report_decisions;
mod report_graph;
mod rtic_pack;
mod rule_packs;
mod shipped_profiles_validate;
mod test_code_scope;
mod toolchains;
mod triad_smoke;
mod typescript_adapter;
mod typescript_items;
mod unit_guidance;
mod validate_layout_dispatch;
mod validate_layout_suppression;
mod validate_parse_failure;
mod validate_pipeline;
mod workspace_migrate;
