//! Deterministic prompt assembly for `specdev gen`.
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
//! The analyzer assembles the prompt; Claude does the
//! emission. This module's output is plumbing, not codegen.

use std::collections::{BTreeMap, BTreeSet};
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

/// List module names (file stems of `specs/modules/*.toml`), lex-sorted.
/// A missing modules dir yields an empty list — a valid cold-start
/// project (conventions + architecture authored, no modules yet).
pub fn list_module_names(layout: &LayoutContext) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    if !layout.modules_dir.is_dir() {
        return Ok(names);
    }
    for entry in std::fs::read_dir(&layout.modules_dir)
        .with_context(|| format!("reading {}", layout.modules_dir.display()))?
    {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Assemble the project-invariant codegen **preamble** (Tier 1 + Tier 2),
/// shared across a set of modules. `/spec-gen` emits this **once** per
/// run; the per-module tail ([`assemble_tail`]) carries everything that
/// varies module-to-module. This is the F-006 fix: the ~400-line
/// conventions prose is no longer re-pasted into every per-module prompt.
///
/// Sections, in fixed order:
///
/// 1. Profile module conventions (`codegen/<lang>/module.conventions.md`) — T1.
/// 2. Per block in the union (lex-sorted): `codegen/<lang>/blocks/<name>/README.md` — T2.
/// 3. Per bridge in the union (lex-sorted): `codegen/<lang>/bridges/<name>/README.md` — T2.
/// 4. Architecture (`specs/architecture.toml`), TOML-fenced — T1.
///
/// `modules` is the set whose block/bridge relevance is unioned: pass one
/// module for the single-module case, the full module list for `--all`,
/// or any subset (the shape a parallel codegen fan-out hands each
/// worker). An empty set yields conventions + architecture only (valid
/// cold-start).
///
/// The preamble carries **no per-module codegen gate** — it is pure
/// shared prose. The gate (`check_codegen_gate`) lives on the per-module
/// tail path. Missing files appear as a parenthetical "(no … found)"
/// note rather than an error, so partial projects still get usable output.
pub fn assemble_preamble(layout: &LayoutContext, modules: &[String]) -> Result<String> {
    // Union of relevant blocks/bridges across the named modules.
    // BTreeSet keeps both lex-ordered and deduped.
    let mut blocks: BTreeSet<String> = BTreeSet::new();
    let mut bridges: BTreeSet<String> = BTreeSet::new();
    for module_name in modules {
        let rel = detect_relevance(layout, module_name)?;
        blocks.extend(rel.blocks);
        bridges.extend(rel.bridges);
    }

    let codegen_dir: PathBuf = layout.codegen_dir.clone();
    let profile_md = codegen_dir.join("module.conventions.md");
    let arch_path = layout.architecture_toml_path.clone();

    let mut out = String::new();

    section_header(&mut out, "Profile module conventions");
    push_file_or_note(&mut out, &profile_md, "no profile module.conventions.md");
    out.push('\n');

    for block_name in &blocks {
        section_header(&mut out, &format!("Block: `{block_name}`"));
        let block_md = codegen_dir.join(format!("blocks/{block_name}/README.md"));
        push_file_or_note(
            &mut out,
            &block_md,
            &format!("no codegen README for block `{block_name}`"),
        );
        out.push('\n');
    }

    for bridge_name in &bridges {
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

    Ok(out)
}

/// Assemble the per-module codegen **tail** (Tier 3): everything that
/// varies module-to-module. Pairs with [`assemble_preamble`]; the
/// concatenation `assemble_preamble(&[m]) + assemble_tail(m)` is the full
/// single-module prompt produced by [`assemble_module_prompt`].
///
/// Sections, in fixed order:
///
/// 1. Module header.
/// 2. Decisions + default-committed OQ context in scope.
/// 3. Project context (`module.context_files`).
/// 4. Test-item verification context.
/// 5. Module spec (`specs/modules/<Module>.toml`), TOML-fenced.
pub fn assemble_tail(layout: &LayoutContext, module_name: &str) -> Result<String> {
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
    let context_files = parse_context_files(&module_toml);

    let mut out = String::new();
    out.push_str(&format!("# Codegen prompt for module `{module_name}`\n\n"));

    // Decisions + default-committed OQs in scope.
    // Walks `specs/decisions.toml` + `specs/open_questions.toml` per-tier,
    // dedupes across (a) module's local guided_by, (b) module's
    // requirements' decided_by, (c) workspace-tier decisions when the
    // module sits inside a workspace member crate. OQs in scope are the
    // ones reachable via Module.satisfies → Requirement →
    // OpenQuestion.affects^-1.
    let decision_oq = collect_decision_oq_context(layout, module_name, &module_toml)?;
    if !decision_oq.is_empty() {
        section_header(&mut out, "Decisions and known unknowns");
        out.push_str(&decision_oq);
        out.push('\n');
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
                    out.push_str(&format!("(failed to read context file `{rel}`: {e})\n",));
                }
            }
            out.push_str("```\n\n");
        }
    }

    // Test-item verification context. For each
    // `[[items]]` entry with `test = true`, resolve its `verifies[]` ids
    // to the requirement text + verification hints (`verified_by`) +
    // the `verification` text of any `decided_by` decision, so
    // the codegen LLM has the criteria to write the test body rather than
    // inventing them. Empty when the module declares no test items.
    let test_ctx = collect_test_item_context(layout, &module_toml)?;
    if !test_ctx.is_empty() {
        section_header(&mut out, "Test items (verification context)");
        out.push_str(&test_ctx);
        out.push('\n');
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

    Ok(out)
}

/// Assemble the full codegen prompt for a single module: the project
/// preamble scoped to this one module, followed by its per-module tail.
/// Equals `assemble_preamble(&[m]) + assemble_tail(m)` by construction.
///
/// Retained as the byte-stable single-module entry point for direct /
/// manual piping. `/spec-gen` uses the split surface instead
/// (`gen --preamble` once + `gen --module <N> --tail` per module) so the
/// preamble prose is emitted once across a multi-module run.
pub fn assemble_module_prompt(layout: &LayoutContext, module_name: &str) -> Result<String> {
    let modules = [module_name.to_string()];
    let mut out = assemble_preamble(layout, &modules)?;
    out.push_str(&assemble_tail(layout, module_name)?);
    // Budget warning measures the effective single-module consumer context
    // (preamble + tail). The split CLI path (`--preamble` / `--tail`)
    // skips this; the skill owns the combined budget there.
    warn_if_over_budget(module_name, out.len());
    Ok(out)
}

/// SHA-256 of a module's whole assembled codegen prompt — the *input* side
/// of the gen tripwire, paired with the *output* `content_hash` recorded by
/// `gen-record`. Hashes exactly [`assemble_module_prompt`] (preamble + tail)
/// so any change to the conventions, architecture, block/bridge prose,
/// in-scope decisions/OQs, context files, or the module spec itself moves the
/// digest. A later run compares this against a freshly-assembled prompt to
/// decide, deterministically, whether the module's generation inputs are
/// unchanged (skippable) or stale (must regenerate). Opaque digest, never
/// source syntax.
pub fn module_input_hash(layout: &LayoutContext, module_name: &str) -> Result<String> {
    let prompt = assemble_module_prompt(layout, module_name)?;
    Ok(crate::hash::content_hash(prompt.as_bytes()))
}

/// [`module_input_hash`] without the budget warning. Same bytes, same
/// digest. For callers that hash every module on every run (`derive`)
/// and would otherwise print one `CONTEXT-003` line per module per gen;
/// the warning belongs to the prompt-emitting path.
pub fn module_input_hash_quiet(layout: &LayoutContext, module_name: &str) -> Result<String> {
    let modules = [module_name.to_string()];
    let mut out = assemble_preamble(layout, &modules)?;
    out.push_str(&assemble_tail(layout, module_name)?);
    Ok(crate::hash::content_hash(out.as_bytes()))
}

/// Build the test-item verification context block.
///
/// Walks the module's `items[]` for entries with `test = true`, and for
/// each resolves its `verifies[]` requirement ids against
/// `specs/requirements.toml` (text + `verified_by` hints) and, through
/// each requirement's `decided_by[]`, the `verification` text of the
/// referenced decisions in `specs/decisions.toml`. Deterministic: items,
/// verifies ids, and decided_by ids are walked in declaration order.
/// Returns an empty string when the module has no test items.
fn collect_test_item_context(layout: &LayoutContext, module_toml: &toml::Value) -> Result<String> {
    let items = module_toml
        .get("items")
        .and_then(toml::Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or(&[]);
    let test_items: Vec<&toml::Value> = items
        .iter()
        .filter(|it| it.get("test").and_then(toml::Value::as_bool) == Some(true))
        .collect();
    if test_items.is_empty() {
        return Ok(String::new());
    }

    // Lazy-load the requirement and decision tables once.
    let reqs = load_toml_array(&layout.specs_dir.join("requirements.toml"), "requirement");
    let decisions = load_toml_array(&layout.specs_dir.join("decisions.toml"), "decision");
    let req_by_id = |id: &str| {
        reqs.iter()
            .find(|r| r.get("id").and_then(toml::Value::as_str) == Some(id))
    };
    let dec_by_id = |id: &str| {
        decisions
            .iter()
            .find(|d| d.get("id").and_then(toml::Value::as_str) == Some(id))
    };

    let mut out = String::new();
    out.push_str(
        "These `items[]` entries are tests. Emit each per its `test_kind` (see the \
         profile conventions). Write a working test body from the verification context \
         below; emit `todo!(\"verifies <ids>\")` only when the criteria are insufficient \
         to write the test.\n\n",
    );

    for item in test_items {
        let name = item
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or("<unnamed>");
        let kind = item
            .get("test_kind")
            .and_then(toml::Value::as_str)
            .unwrap_or("unit");
        out.push_str(&format!("### `{name}` (test_kind = `{kind}`)\n"));
        if let Some(desc) = item.get("description").and_then(toml::Value::as_str) {
            out.push_str(&format!("{desc}\n"));
        }
        let verifies = item
            .get("verifies")
            .and_then(toml::Value::as_array)
            .map(|a| a.as_slice())
            .unwrap_or(&[]);
        for vid in verifies.iter().filter_map(toml::Value::as_str) {
            match req_by_id(vid) {
                Some(req) => {
                    let text = req.get("text").and_then(toml::Value::as_str).unwrap_or("");
                    out.push_str(&format!("- **{vid}**: {text}\n"));
                    if let Some(vb) = req.get("verified_by").and_then(toml::Value::as_array) {
                        let hints: Vec<&str> = vb.iter().filter_map(toml::Value::as_str).collect();
                        if !hints.is_empty() {
                            out.push_str(&format!(
                                "  - verification hints: {}\n",
                                hints.join("; ")
                            ));
                        }
                    }
                    if let Some(db) = req.get("decided_by").and_then(toml::Value::as_array) {
                        for did in db.iter().filter_map(toml::Value::as_str) {
                            if let Some(v) = dec_by_id(did)
                                .and_then(|d| d.get("verification"))
                                .and_then(toml::Value::as_str)
                            {
                                out.push_str(&format!("  - {did} verification: {v}\n"));
                            }
                        }
                    }
                }
                None => {
                    out.push_str(&format!("- **{vid}**: (requirement not found)\n"));
                }
            }
        }
        out.push('\n');
    }
    Ok(out)
}

/// Best-effort load of a top-level array-of-tables from a TOML file.
/// Missing / unparseable files yield an empty vec.
fn load_toml_array(path: &std::path::Path, key: &str) -> Vec<toml::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
        .and_then(|v| v.get(key).and_then(toml::Value::as_array).cloned())
        .unwrap_or_default()
}

/// Codegen-gate refusal returned by [`check_codegen_gate`] when an open
/// OpenQuestion is reachable from the target module via Module.satisfies
/// → Requirement → OpenQuestion.affects^-1.
///
/// `message` carries the verbatim refusal text (the three-exit-options
/// block) ready to print to stderr; `oq_id` and `oq_title`
/// are surfaced separately for tests + structured consumers.
#[derive(Debug, Clone)]
pub struct GateRefusal {
    pub module: String,
    pub oq_id: String,
    pub oq_title: String,
    pub message: String,
}

/// Codegen gate.
///
/// For module `module_name`, walk `module.satisfies` → Requirement →
/// `open_question_affects_requirement`-reverse and check whether any
/// reachable OpenQuestion has `status = open`. Returns `Ok(None)` when
/// the module is unblocked (no reachable open OQ), `Ok(Some(refusal))`
/// when blocked. The refusal carries the literal text the user must see
/// at gen time (three exit options + pointer to specs/open_questions.toml).
///
/// Workspace-tier OQs are also consulted when the project sits in a
/// workspace (mirrors the prompt walk's source set).
pub fn check_codegen_gate(
    layout: &LayoutContext,
    module_name: &str,
) -> Result<Option<GateRefusal>> {
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
    let satisfies: Vec<String> = module_toml
        .get("satisfies")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let local_oqs = read_open_questions(&layout.specs_dir.join("open_questions.toml"))?;
    let mut workspace_oqs: BTreeMap<String, OpenQuestionEntry> = BTreeMap::new();
    let mut workspace_specs: Option<std::path::PathBuf> = None;
    if let Some(parent) = layout.project_root.parent() {
        let parent_specs = parent.join("specs");
        let parent_spec_toml = parent.join("spec.toml");
        if parent_spec_toml.is_file() {
            if let Ok(text) = std::fs::read_to_string(&parent_spec_toml) {
                if let Ok(parsed) = toml::from_str::<toml::Value>(&text) {
                    if parsed.get("workspace").is_some() {
                        workspace_oqs =
                            read_open_questions(&parent_specs.join("open_questions.toml"))?;
                        workspace_specs = Some(parent_specs);
                    }
                }
            }
        }
    }

    // Local-tier OQs gate when their affects[] overlaps the module's
    // satisfies[]. Iteration is deterministic via BTreeMap ordering.
    for (oq_id, entry) in &local_oqs {
        if entry.status != "open" {
            continue;
        }
        let intersects = entry.affects.iter().any(|r| satisfies.contains(r));
        if !intersects {
            continue;
        }
        let title = read_open_question_title(&layout.specs_dir.join("open_questions.toml"), oq_id)
            .unwrap_or_default();
        let message = build_refusal_message(
            module_name,
            oq_id,
            &title,
            "the project's `specs/open_questions.toml`",
        );
        return Ok(Some(GateRefusal {
            module: module_name.to_string(),
            oq_id: oq_id.clone(),
            oq_title: title,
            message,
        }));
    }

    // Workspace-tier OQs implicitly apply down. When `affects` is
    // empty/absent the OQ blocks every
    // module in every member crate; when populated, it acts like a
    // local OQ with explicit scope. Local-tier OQs with the same id
    // take precedence over workspace entries.
    for (oq_id, entry) in &workspace_oqs {
        if local_oqs.contains_key(oq_id) {
            continue;
        }
        if entry.status != "open" {
            continue;
        }
        let intersects =
            entry.affects.is_empty() || entry.affects.iter().any(|r| satisfies.contains(r));
        if !intersects {
            continue;
        }
        let title = workspace_specs
            .as_ref()
            .and_then(|p| read_open_question_title(&p.join("open_questions.toml"), oq_id))
            .unwrap_or_default();
        let message = build_refusal_message(
            module_name,
            oq_id,
            &title,
            "the workspace's `specs/open_questions.toml`",
        );
        return Ok(Some(GateRefusal {
            module: module_name.to_string(),
            oq_id: oq_id.clone(),
            oq_title: title,
            message,
        }));
    }

    Ok(None)
}

fn build_refusal_message(module_name: &str, oq_id: &str, title: &str, pointer: &str) -> String {
    format!(
        "Refusing to generate code for module `{module_name}`: open question \
         `{oq_id}` (`{title}`) is unresolved.\n\n\
         Three exit options:\n\
         \x20\x201. Resolve via a decision: record a status=accepted Decision \
         that answers the question, list it in the OQ's `closed_by`, and set \
         the OQ's status to `closed`.\n\
         \x20\x202. Commit a default: change the OQ's status to \
         `default_committed`, set `default_via = \"DEC-...\"` referencing an \
         accepted Decision, and provide a `verify = \"...\"` placeholder.\n\
         \x20\x203. Mark the question moot: record a status=accepted Decision \
         that lists the OQ in `closes_questions`, and set the OQ's status to \
         `closed`.\n\n\
         See {pointer}.\n",
    )
}

fn read_open_question_title(path: &std::path::Path, oq_id: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: toml::Value = toml::from_str(&text).ok()?;
    let arr = parsed
        .get("open_question")
        .and_then(toml::Value::as_array)?;
    for item in arr {
        if item.get("id").and_then(toml::Value::as_str) == Some(oq_id) {
            return item
                .get("title")
                .and_then(toml::Value::as_str)
                .map(str::to_string);
        }
    }
    None
}

/// Collect decisions + default_committed/closed OQ context for a module.
/// Three sources, deduped:
///
/// (a) module.guided_by (each id resolves to a Decision)
/// (b) requirements satisfied by the module → each requirement.decided_by
/// (c) workspace-tier decisions when this project sits inside a workspace
///
/// Output is sorted by id for byte-stability. Status=`superseded`
/// decisions are skipped. OQs are surfaced when they affect any
/// of the module's satisfied requirements:
///   - status = default_committed → "(working assumption)" + verify text
///   - status = closed             → "(resolved)" + pointer to closed_by
///   - status = open               → omitted at prompt time (the gen gate
///     refuses before reaching this code path; for `prompt` (no-gate)
///     surface, the open OQ is the loud signal that gen will refuse).
pub fn collect_decision_oq_context(
    layout: &LayoutContext,
    module_name: &str,
    module_toml: &toml::Value,
) -> Result<String> {
    use std::collections::BTreeMap;

    // -- gather requirements + decisions + OQs per tier ---------------------

    // Per-project requirements + sources file → list of requirement
    // (id, decided_by). The module is rooted at `layout.project_root`;
    // we read this project's requirements.toml as primary.
    let satisfies: Vec<String> = module_toml
        .get("satisfies")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    // Local decisions/oqs.
    let local_decisions = read_decisions(&layout.specs_dir.join("decisions.toml"))?;
    let local_oqs = read_open_questions(&layout.specs_dir.join("open_questions.toml"))?;

    // Workspace-tier decisions/oqs. The workspace's
    // specs/decisions.toml lives one level up from a member crate's
    // project_root when the project sits inside a `<workspace>/<member>/`
    // shape. The detection heuristic: a workspace's spec.toml lives at
    // `<parent>/spec.toml` and declares `[workspace]`. We probe the
    // parent directory; if `<parent>/specs/decisions.toml` exists, treat
    // it as workspace tier.
    let mut workspace_decisions: BTreeMap<String, DecisionEntry> = BTreeMap::new();
    let mut workspace_oqs: BTreeMap<String, OpenQuestionEntry> = BTreeMap::new();
    if let Some(parent) = layout.project_root.parent() {
        let parent_specs = parent.join("specs");
        let parent_dec = parent_specs.join("decisions.toml");
        let parent_spec_toml = parent.join("spec.toml");
        if parent_dec.is_file() && parent_spec_toml.is_file() {
            // Confirm this parent is a workspace.
            if let Ok(text) = std::fs::read_to_string(&parent_spec_toml) {
                if let Ok(parsed) = toml::from_str::<toml::Value>(&text) {
                    if parsed.get("workspace").is_some() {
                        for (k, v) in read_decisions(&parent_dec)? {
                            workspace_decisions.insert(k, v);
                        }
                        for (k, v) in
                            read_open_questions(&parent_specs.join("open_questions.toml"))?
                        {
                            workspace_oqs.insert(k, v);
                        }
                    }
                }
            }
        }
    }

    // Per-project requirements: id → decided_by[].
    let req_decided_by = read_requirement_decided_by(&layout.requirements_toml_path)?;
    // Per-project OQs: keyed by (id → entry), already in local_oqs.
    // Per-project requirement → affecting OQs (reverse of OQ.affects).
    let req_to_oqs = build_req_to_oqs(&local_oqs);

    // -- compute the in-scope decision id set -----------------------------

    let mut in_scope_decisions: BTreeMap<String, DecisionEntry> = BTreeMap::new();
    let mut emit_decision = |id: &str, decisions: &BTreeMap<String, DecisionEntry>| {
        if let Some(d) = decisions.get(id) {
            if d.status == "superseded" {
                return;
            }
            in_scope_decisions.insert(id.to_string(), d.clone());
        }
    };

    // (a) module.guided_by
    let guided_by: Vec<String> = module_toml
        .get("guided_by")
        .and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for id in &guided_by {
        emit_decision(id, &local_decisions);
        emit_decision(id, &workspace_decisions);
    }

    // (b) module.satisfies → Requirement.decided_by
    for req_id in &satisfies {
        let Some(decision_ids) = req_decided_by.get(req_id) else {
            continue;
        };
        for id in decision_ids {
            emit_decision(id, &local_decisions);
            emit_decision(id, &workspace_decisions);
        }
    }

    // (c) workspace-tier decisions
    for (id, entry) in &workspace_decisions {
        if entry.status == "superseded" {
            continue;
        }
        in_scope_decisions.insert(id.clone(), entry.clone());
    }

    // -- compute in-scope OQs ---------------------------------------------

    let mut in_scope_oqs: BTreeMap<String, OpenQuestionEntry> = BTreeMap::new();
    for req_id in &satisfies {
        if let Some(oq_ids) = req_to_oqs.get(req_id) {
            for oq_id in oq_ids {
                if let Some(entry) = local_oqs.get(oq_id) {
                    in_scope_oqs.insert(oq_id.clone(), entry.clone());
                }
            }
        }
    }
    // Workspace-tier OQs rarely apply to a member module (no requirements
    // at workspace tier in the reduction model), but include any OQ
    // whose `affects` array names a satisfied requirement.
    for (oq_id, entry) in &workspace_oqs {
        for affects in &entry.affects {
            if satisfies.iter().any(|s| s == affects) {
                in_scope_oqs.insert(oq_id.clone(), entry.clone());
            }
        }
    }

    // -- render -----------------------------------------------------------

    let mut out = String::new();
    if in_scope_decisions.is_empty() && in_scope_oqs.is_empty() {
        return Ok(out);
    }

    // Module header.
    out.push_str(&format!("Context for module `{module_name}`:\n\n"));

    if !in_scope_decisions.is_empty() {
        for (id, d) in &in_scope_decisions {
            out.push_str(&format!("Decision {id}: {}.\n{}\n", d.title, d.text));
            if let Some(c) = &d.consequences {
                out.push_str(&format!("Consequences: {c}\n"));
            }
            out.push('\n');
        }
    }

    if !in_scope_oqs.is_empty() {
        for (id, oq) in &in_scope_oqs {
            match oq.status.as_str() {
                "default_committed" => {
                    out.push_str(&format!(
                        "Open question {id} (working assumption): {}.",
                        oq.text
                    ));
                    if let Some(v) = &oq.verify {
                        out.push_str(&format!(" Verify by: {v}"));
                    }
                    out.push('\n');
                }
                "closed" => {
                    let cb = oq.closed_by.as_deref().unwrap_or("(unknown)");
                    out.push_str(&format!(
                        "Open question {id} (resolved): see Decision {cb}.\n"
                    ));
                }
                "open" => {
                    out.push_str(&format!(
                        "Open question {id} (UNRESOLVED — gen will refuse): {}.\n",
                        oq.text
                    ));
                }
                _ => {}
            }
        }
    }

    Ok(out)
}

#[derive(Debug, Clone)]
struct DecisionEntry {
    title: String,
    status: String,
    text: String,
    consequences: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct OpenQuestionEntry {
    pub(crate) status: String,
    pub(crate) text: String,
    pub(crate) closed_by: Option<String>,
    pub(crate) verify: Option<String>,
    pub(crate) affects: Vec<String>,
}

fn read_decisions(
    path: &std::path::Path,
) -> Result<std::collections::BTreeMap<String, DecisionEntry>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, DecisionEntry> = BTreeMap::new();
    if !path.is_file() {
        return Ok(out);
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(arr) = parsed.get("decision").and_then(toml::Value::as_array) {
        for item in arr {
            let Some(id) = item.get("id").and_then(toml::Value::as_str) else {
                continue;
            };
            let title = item
                .get("title")
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string();
            let status = item
                .get("status")
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string();
            let text = item
                .get("text")
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string();
            let consequences = item
                .get("consequences")
                .and_then(toml::Value::as_str)
                .map(str::to_string);
            out.insert(
                id.to_string(),
                DecisionEntry {
                    title,
                    status,
                    text,
                    consequences,
                },
            );
        }
    }
    Ok(out)
}

pub(crate) fn read_open_questions(
    path: &std::path::Path,
) -> Result<std::collections::BTreeMap<String, OpenQuestionEntry>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, OpenQuestionEntry> = BTreeMap::new();
    if !path.is_file() {
        return Ok(out);
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(arr) = parsed.get("open_question").and_then(toml::Value::as_array) {
        for item in arr {
            let Some(id) = item.get("id").and_then(toml::Value::as_str) else {
                continue;
            };
            let status = item
                .get("status")
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string();
            let text = item
                .get("text")
                .and_then(toml::Value::as_str)
                .unwrap_or("")
                .to_string();
            let closed_by = item
                .get("closed_by")
                .and_then(toml::Value::as_str)
                .map(str::to_string);
            let verify = item
                .get("verify")
                .and_then(toml::Value::as_str)
                .map(str::to_string);
            let affects: Vec<String> = item
                .get("affects")
                .and_then(toml::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            out.insert(
                id.to_string(),
                OpenQuestionEntry {
                    status,
                    text,
                    closed_by,
                    verify,
                    affects,
                },
            );
        }
    }
    Ok(out)
}

fn read_requirement_decided_by(
    path: &std::path::Path,
) -> Result<std::collections::BTreeMap<String, Vec<String>>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if !path.is_file() {
        return Ok(out);
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let parsed: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if let Some(arr) = parsed.get("requirement").and_then(toml::Value::as_array) {
        for item in arr {
            let Some(id) = item.get("id").and_then(toml::Value::as_str) else {
                continue;
            };
            let decisions: Vec<String> = item
                .get("decided_by")
                .and_then(toml::Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            out.insert(id.to_string(), decisions);
        }
    }
    Ok(out)
}

fn build_req_to_oqs(
    oqs: &std::collections::BTreeMap<String, OpenQuestionEntry>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (oq_id, entry) in oqs {
        for req_id in &entry.affects {
            out.entry(req_id.clone()).or_default().push(oq_id.clone());
        }
    }
    out
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
    let threshold = ((budget_chars as f64) * BUDGET_WARNING_FRACTION) as usize;
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
