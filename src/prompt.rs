//! Phase 8: deterministic prompt assembly for `specdev gen`.
//!
//! Reads a module spec + the inlined blocks/bridges' codegen prose +
//! the project's architecture.toml, and concatenates them into a
//! single string the plugin's `spec-gen` skill hands to Claude. The
//! assembly is **deterministic by construction**: relevant blocks and
//! bridges are detected from `lineage.toml` (the source of truth for
//! what's been inlined) intersected with the module's authored keys,
//! then emitted in lex order. The same project + module produces a
//! byte-identical prompt across runs.
//!
//! Per D29: the analyzer assembles the prompt; Claude does the
//! emission. This module's output is plumbing, not codegen.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use tracing::warn;

use crate::layout::LayoutContext;
use crate::lineage::Lineage;

/// Default token-budget approximation used by the soft-warning path.
/// 200k tokens × ~4 chars/token ≈ 800k chars; 80% threshold = 640k chars.
/// Override via `SPECDEV_TOKEN_BUDGET` (in tokens, not chars).
const DEFAULT_TOKEN_BUDGET: usize = 200_000;
const APPROX_CHARS_PER_TOKEN: usize = 4;
const BUDGET_WARNING_FRACTION: f64 = 0.80;

/// Read `module.context_files` from a parsed module TOML in declared
/// order. Returns an empty list when the field is absent or empty.
pub fn parse_context_files(module_toml: &toml::Value) -> Vec<String> {
    module_toml
        .get("context_files")
        .and_then(toml::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Result of relevance detection: the inlined blocks and bridges this
/// module references. Both lists are lex-sorted; bridges by full
/// `<a>+<b>` name, blocks by plugin-tree directory name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Relevance {
    /// Block names (plugin-tree filenames, e.g. `embassy`, `stm32h5-rs`).
    pub blocks: Vec<String>,
    /// Bridge names (lex-ordered `<a>+<b>`, e.g. `embassy+stm32h5-rs`).
    pub bridges: Vec<String>,
}

/// Detect which inlined blocks and bridges a module's authored keys
/// reference.
///
/// A block is "relevant" if the module's top-level keys include any
/// property the block's schema declares at top level. (Block plugin
/// names and module-TOML keys differ — `stm32h5-rs` block uses key
/// `stm32h5`. The schema is the source of truth.)
///
/// A bridge is "relevant" if the module has a `bindings.<bridge-name>`
/// table where `<bridge-name>` matches the bridge's lineage entry.
pub fn detect_relevance(layout: &LayoutContext, module_name: &str) -> Result<Relevance> {
    let module_path = layout.modules_dir.join(format!("{module_name}.toml"));
    if !module_path.is_file() {
        return Err(anyhow!(
            "module `{module_name}` not found at {}",
            module_path.display(),
        ));
    }
    let module_text = std::fs::read_to_string(&module_path)
        .with_context(|| format!("reading {}", module_path.display()))?;
    let module_toml: toml::Value = toml::from_str(&module_text)
        .with_context(|| format!("parsing {}", module_path.display()))?;

    let module_keys: BTreeSet<String> = match &module_toml {
        toml::Value::Table(t) => t.keys().cloned().collect(),
        _ => BTreeSet::new(),
    };

    let lineage = Lineage::load(&layout.specdev_root)?;

    // Block relevance: read each inlined block's schema, get its
    // top-level property names, intersect with the module's keys.
    let mut blocks: Vec<String> = Vec::new();
    let mut block_names: Vec<String> = lineage.blocks.keys().cloned().collect();
    block_names.sort();
    for block_name in &block_names {
        let schema_path = layout
            .specdev_root
            .join(format!("schemas/inlined/blocks/{block_name}.schema.yaml"));
        if !schema_path.is_file() {
            continue;
        }
        let schema_text = std::fs::read_to_string(&schema_path)
            .with_context(|| format!("reading {}", schema_path.display()))?;
        let schema_value: Value = serde_yaml::from_str(&schema_text)
            .with_context(|| format!("parsing {}", schema_path.display()))?;
        let Some(props) = schema_value.get("properties").and_then(Value::as_object) else {
            continue;
        };
        for key in props.keys() {
            if module_keys.contains(key) {
                blocks.push(block_name.clone());
                break;
            }
        }
    }

    // Bridge relevance: a `[bindings.<bridge-name>]` sub-table referencing
    // an inlined bridge.
    let mut bridges: Vec<String> = Vec::new();
    let mut bridge_names: Vec<String> = lineage.bridges.keys().cloned().collect();
    bridge_names.sort();
    if let Some(bindings) = module_toml.get("bindings").and_then(toml::Value::as_table) {
        for bridge_name in &bridge_names {
            if bindings.contains_key(bridge_name) {
                bridges.push(bridge_name.clone());
            }
        }
    }

    Ok(Relevance { blocks, bridges })
}

/// Assemble the codegen prompt for a single module.
///
/// Sections, in fixed order:
///
/// 1. Profile module conventions (`codegen/<lang>/module.conventions.md`).
/// 2. Per relevant block (lex-sorted): `codegen/<lang>/blocks/<name>/README.md`.
/// 3. Per relevant bridge (lex-sorted): `codegen/<lang>/bridges/<name>/README.md`.
/// 4. Architecture (`specs/architecture.toml`), TOML-fenced.
/// 5. Module spec (`specs/modules/<Module>.toml`), TOML-fenced.
///
/// Missing files appear as a parenthetical "(no … found)" note rather
/// than an error — partial projects (e.g., a block whose codegen prose
/// hasn't been authored) still get a usable prompt; the gap is visible
/// in the output.
pub fn assemble_module_prompt(layout: &LayoutContext, module_name: &str) -> Result<String> {
    let relevance = detect_relevance(layout, module_name)?;

    let module_path = layout.modules_dir.join(format!("{module_name}.toml"));
    let module_text = std::fs::read_to_string(&module_path)
        .with_context(|| format!("reading {}", module_path.display()))?;
    let module_toml: toml::Value = toml::from_str(&module_text)
        .with_context(|| format!("parsing {}", module_path.display()))?;
    let context_files = parse_context_files(&module_toml);

    let codegen_dir: PathBuf = layout.codegen_dir.clone();
    let profile_md = codegen_dir.join("module.conventions.md");
    let arch_path = layout.architecture_toml_path.clone();

    let mut out = String::new();
    out.push_str(&format!("# Codegen prompt for module `{module_name}`\n\n"));

    section_header(&mut out, "Profile module conventions");
    push_file_or_note(&mut out, &profile_md, "no profile module.conventions.md");
    out.push('\n');

    for block_name in &relevance.blocks {
        section_header(&mut out, &format!("Block: `{block_name}`"));
        let block_md = codegen_dir.join(format!("blocks/{block_name}/README.md"));
        push_file_or_note(
            &mut out,
            &block_md,
            &format!("no codegen README for block `{block_name}`"),
        );
        out.push('\n');
    }

    for bridge_name in &relevance.bridges {
        section_header(&mut out, &format!("Bridge: `{bridge_name}`"));
        let bridge_md = codegen_dir.join(format!("bridges/{bridge_name}/README.md"));
        push_file_or_note(
            &mut out,
            &bridge_md,
            &format!("no codegen README for bridge `{bridge_name}`"),
        );
        out.push('\n');
    }

    section_header(&mut out, "Architecture (`specs/architecture.toml`)");
    if arch_path.is_file() {
        out.push_str("```toml\n");
        let s = std::fs::read_to_string(&arch_path)
            .with_context(|| format!("reading {}", arch_path.display()))?;
        out.push_str(&s);
        if !s.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("```\n\n");
    } else {
        out.push_str("(no architecture.toml)\n\n");
    }

    if !context_files.is_empty() {
        section_header(&mut out, "Project context");
        for rel in &context_files {
            let abs = layout.project_root.join(rel);
            out.push_str(&format!("```{rel}\n"));
            match std::fs::read_to_string(&abs) {
                Ok(s) => {
                    out.push_str(&s);
                    if !s.ends_with('\n') {
                        out.push('\n');
                    }
                }
                Err(e) => {
                    out.push_str(&format!(
                        "(failed to read context file `{rel}`: {e})\n",
                    ));
                }
            }
            out.push_str("```\n\n");
        }
    }

    section_header(
        &mut out,
        &format!("Module spec (`specs/modules/{module_name}.toml`)"),
    );
    out.push_str("```toml\n");
    out.push_str(&module_text);
    if !module_text.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n");

    warn_if_over_budget(module_name, out.len());

    Ok(out)
}

/// Emit a `tracing::warn!` when the assembled prompt exceeds 80% of the
/// configured token budget (approximated as `chars / 4`). Override the
/// budget via the `SPECDEV_TOKEN_BUDGET` env var, in tokens.
fn warn_if_over_budget(module_name: &str, char_count: usize) {
    let budget_tokens = std::env::var("SPECDEV_TOKEN_BUDGET")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_TOKEN_BUDGET);
    let budget_chars = budget_tokens.saturating_mul(APPROX_CHARS_PER_TOKEN);
    let threshold =
        ((budget_chars as f64) * BUDGET_WARNING_FRACTION) as usize;
    if char_count >= threshold {
        let approx_tokens = char_count / APPROX_CHARS_PER_TOKEN;
        warn!(
            target: "specdev_analyzer::prompt",
            "[CONTEXT-003] codegen prompt for module `{module_name}` is \
             approximately {approx_tokens} tokens ({char_count} chars), \
             at or above 80% of the {budget_tokens}-token budget — \
             consider trimming `module.context_files` or splitting per-topic"
        );
    }
}

fn section_header(out: &mut String, title: &str) {
    out.push_str("---\n\n## ");
    out.push_str(title);
    out.push_str("\n\n");
}

fn push_file_or_note(out: &mut String, path: &std::path::Path, missing_note: &str) {
    if path.is_file() {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                out.push_str(&s);
                if !s.ends_with('\n') {
                    out.push('\n');
                }
            }
            Err(_) => {
                out.push_str(&format!("({missing_note} — read failed)\n"));
            }
        }
    } else {
        out.push_str(&format!("({missing_note})\n"));
    }
}
