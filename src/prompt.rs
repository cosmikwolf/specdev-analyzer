//! Per-module context over the spec: which inlined blocks and bridges a
//! module uses ([`detect_relevance`]), the module list, the decisions and
//! open questions that reach a module and why ([`decision_context`]), and
//! the open-question gate ([`check_codegen_gate`]) that refuses work on a
//! module an open question still blocks.
//!
//! Relevance is detected from `lineage.toml` (what has been inlined)
//! intersected with the module's authored keys, in lex order, so the same
//! project and module give the same answer on every run.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::entries::{EntryKind, EntryStore};
use crate::layout::LayoutContext;
use crate::lineage::Lineage;

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

    let local_store = EntryStore::for_layout(layout);
    let local_oqs = read_open_questions(&local_store)?;
    // The workspace root, found by its `[workspace].members[].path` listing
    // this project, read in the root's own layout.
    let workspace_store = EntryStore::workspace_root_of(layout)?;
    let workspace_oqs: BTreeMap<String, OpenQuestionEntry> = match &workspace_store {
        Some(store) => read_open_questions(store)?,
        None => BTreeMap::new(),
    };

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
        let title = read_open_question_title(&local_store, oq_id).unwrap_or_default();
        let message = build_refusal_message(
            module_name,
            oq_id,
            &title,
            &format!(
                "the project's {}",
                local_store.location_hint(EntryKind::OpenQuestion, oq_id)
            ),
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
        // Non-empty `workspace_oqs` implies a workspace store.
        let Some(store) = workspace_store.as_ref() else {
            continue;
        };
        let title = read_open_question_title(store, oq_id).unwrap_or_default();
        let message = build_refusal_message(
            module_name,
            oq_id,
            &title,
            &format!(
                "the workspace's {}",
                store.location_hint(EntryKind::OpenQuestion, oq_id)
            ),
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
        "Module `{module_name}` is blocked: open question `{oq_id}` (`{title}`) is \
         unresolved, and code for it exists. Resolve the question before building on it.\n\n\
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

fn read_open_question_title(store: &EntryStore, oq_id: &str) -> Option<String> {
    for entry in store.load(EntryKind::OpenQuestion).ok()? {
        if entry.id() == Some(oq_id) {
            return entry
                .value
                .get("title")
                .and_then(toml::Value::as_str)
                .map(str::to_string);
        }
    }
    None
}

/// Why a decision or open question reaches a module's prompt.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "via", content = "id")]
pub enum ContextReason {
    /// Named in the module's own `guided_by`.
    GuidedBy,
    /// The `decided_by` of a requirement the module `satisfies`.
    Requirement(String),
    /// A workspace-tier decision: it reaches every member module.
    Workspace,
    /// An open question whose `affects` names a satisfied requirement.
    Affects(String),
}

/// One decision in a module's prompt.
#[derive(Debug, Clone)]
pub struct ScopedDecision {
    pub id: String,
    pub title: String,
    /// `true` when the entry was read from the workspace root's file.
    pub workspace: bool,
    /// The prose fields that reach the prompt, in render order.
    pub fields: Vec<&'static str>,
    pub reasons: Vec<ContextReason>,
}

/// One open question in a module's prompt.
#[derive(Debug, Clone)]
pub struct ScopedQuestion {
    pub id: String,
    pub status: String,
    pub reasons: Vec<ContextReason>,
}

/// The decisions and open questions that reach one module's prompt, and
/// why. Computed once; [`collect_decision_oq_context`] renders it into the
/// prompt, and `report decisions` prints it (the only view of "which
/// decision text does this module's prompt carry" short of assembling the
/// prompt and searching it).
#[derive(Debug, Clone, Default)]
pub struct DecisionContext {
    /// By id.
    pub decisions: Vec<ScopedDecision>,
    /// By id.
    pub open_questions: Vec<ScopedQuestion>,
}

/// Compute a module's [`DecisionContext`]. Three decision sources,
/// deduplicated: (a) `guided_by`, (b) `satisfies` → requirement
/// `decided_by`, (c) every workspace-tier decision. Superseded decisions are
/// skipped. A decision id present in both tiers takes the workspace entry.
pub fn decision_context(
    layout: &LayoutContext,
    module_toml: &toml::Value,
) -> Result<DecisionContext> {
    use std::collections::BTreeMap;

    let strings = |key: &str| -> Vec<String> {
        module_toml
            .get(key)
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let satisfies = strings("satisfies");
    let guided_by = strings("guided_by");

    // Local decisions/oqs.
    let local_store = EntryStore::for_layout(layout);
    let local_decisions = read_decisions(&local_store)?;
    let local_oqs = read_open_questions(&local_store)?;

    // Workspace-tier decisions/oqs: the root whose `[workspace].members[]`
    // lists this project, at any depth (a member at `crates/<m>` included),
    // read in the root's own layout. Open questions load whether or not the
    // root holds any decision.
    let mut workspace_decisions: BTreeMap<String, DecisionEntry> = BTreeMap::new();
    let mut workspace_oqs: BTreeMap<String, OpenQuestionEntry> = BTreeMap::new();
    if let Some(store) = EntryStore::workspace_root_of(layout)? {
        workspace_decisions = read_decisions(&store)?;
        workspace_oqs = read_open_questions(&store)?;
    }

    // Per-project requirements: id → decided_by[].
    let req_decided_by = read_requirement_decided_by(&local_store)?;
    // Per-project requirement → affecting OQs (reverse of OQ.affects).
    let req_to_oqs = build_req_to_oqs(&local_oqs);

    // -- the in-scope decision set: (entry, from workspace?, reasons) ------

    let mut in_scope: BTreeMap<String, (DecisionEntry, bool, Vec<ContextReason>)> = BTreeMap::new();
    // A later insert of the same id replaces the entry (workspace after
    // local) and keeps the reasons gathered so far.
    let mut emit = |id: &str, d: &DecisionEntry, workspace: bool, why: ContextReason| {
        if d.status == "superseded" {
            return;
        }
        let slot = in_scope
            .entry(id.to_string())
            .or_insert_with(|| (d.clone(), workspace, Vec::new()));
        slot.0 = d.clone();
        slot.1 = workspace;
        if !slot.2.contains(&why) {
            slot.2.push(why);
        }
    };

    // (a) module.guided_by
    for id in &guided_by {
        for (tier, workspace) in [(&local_decisions, false), (&workspace_decisions, true)] {
            if let Some(d) = tier.get(id) {
                emit(id, d, workspace, ContextReason::GuidedBy);
            }
        }
    }

    // (b) module.satisfies → Requirement.decided_by
    for req_id in &satisfies {
        let Some(decision_ids) = req_decided_by.get(req_id) else {
            continue;
        };
        for id in decision_ids {
            for (tier, workspace) in [(&local_decisions, false), (&workspace_decisions, true)] {
                if let Some(d) = tier.get(id) {
                    emit(id, d, workspace, ContextReason::Requirement(req_id.clone()));
                }
            }
        }
    }

    // (c) workspace-tier decisions
    for (id, entry) in &workspace_decisions {
        emit(id, entry, true, ContextReason::Workspace);
    }

    // -- in-scope OQs -----------------------------------------------------

    let mut oqs: BTreeMap<String, (OpenQuestionEntry, Vec<ContextReason>)> = BTreeMap::new();
    let mut emit_oq = |id: &str, e: &OpenQuestionEntry, why: ContextReason| {
        let slot = oqs
            .entry(id.to_string())
            .or_insert_with(|| (e.clone(), Vec::new()));
        slot.0 = e.clone();
        if !slot.1.contains(&why) {
            slot.1.push(why);
        }
    };
    for req_id in &satisfies {
        if let Some(oq_ids) = req_to_oqs.get(req_id) {
            for oq_id in oq_ids {
                if let Some(entry) = local_oqs.get(oq_id) {
                    emit_oq(oq_id, entry, ContextReason::Affects(req_id.clone()));
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
                emit_oq(oq_id, entry, ContextReason::Affects(affects.clone()));
            }
        }
    }

    Ok(DecisionContext {
        decisions: in_scope
            .into_iter()
            .map(|(id, (entry, workspace, reasons))| ScopedDecision {
                id,
                title: entry.title.clone(),
                workspace,
                fields: entry.prose.iter().map(|(f, _, _)| *f).collect(),
                reasons,
            })
            .collect(),
        open_questions: oqs
            .into_iter()
            .map(|(id, (entry, reasons))| ScopedQuestion {
                id,
                status: entry.status.clone(),
                reasons,
            })
            .collect(),
    })
}

/// Every decision this project's prompts could carry: its own, then the
/// workspace root's. `(id, title, status, from the workspace?)`, by id per
/// tier.
pub fn project_decisions(layout: &LayoutContext) -> Result<Vec<(String, String, String, bool)>> {
    let mut out: Vec<(String, String, String, bool)> =
        read_decisions(&EntryStore::for_layout(layout))?
            .into_iter()
            .map(|(id, d)| (id, d.title, d.status, false))
            .collect();
    if let Some(store) = EntryStore::workspace_root_of(layout)? {
        out.extend(
            read_decisions(&store)?
                .into_iter()
                .map(|(id, d)| (id, d.title, d.status, true)),
        );
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct DecisionEntry {
    title: String,
    status: String,
    /// The prose fields the prompt inlines, in [`DECISION_PROSE_FIELDS`]
    /// order, each `(field, label, value)`. Selected by the decision's
    /// `codegen_include`; `[text, consequences]` when it is absent.
    prose: Vec<(&'static str, &'static str, String)>,
}

/// The decision prose fields `codegen_include` may name, in render order,
/// with the label each is printed under (`text` is printed bare, under the
/// title line). Render order is fixed, not the order the author listed, so
/// the prompt stays byte-stable when the list is reordered.
const DECISION_PROSE_FIELDS: [(&str, &str); 5] = [
    ("text", ""),
    ("context", "Context"),
    ("consequences", "Consequences"),
    ("alternatives", "Alternatives"),
    ("verification", "Verification"),
];

/// `codegen_include` when a decision omits it — the schema's documented
/// default, and byte-for-byte what the prompt carried before the field was
/// honoured.
const DECISION_DEFAULT_INCLUDE: [&str; 2] = ["text", "consequences"];

#[derive(Debug, Clone)]
pub(crate) struct OpenQuestionEntry {
    pub(crate) status: String,
    pub(crate) affects: Vec<String>,
}

fn read_decisions(store: &EntryStore) -> Result<std::collections::BTreeMap<String, DecisionEntry>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, DecisionEntry> = BTreeMap::new();
    for entry in store.load(EntryKind::Decision)? {
        let item = &entry.value;
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
        let include: Vec<String> = match item.get("codegen_include").and_then(toml::Value::as_array)
        {
            Some(arr) => arr
                .iter()
                .filter_map(toml::Value::as_str)
                .map(str::to_string)
                .collect(),
            None => DECISION_DEFAULT_INCLUDE
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        let prose = DECISION_PROSE_FIELDS
            .iter()
            .filter(|(field, _)| include.iter().any(|i| i == field))
            .filter_map(|(field, label)| {
                // `text` is required by the schema; an absent one still
                // prints as an empty body, as it always has.
                let value = item.get(*field).and_then(toml::Value::as_str);
                match (*field, value) {
                    ("text", v) => Some((*field, *label, v.unwrap_or("").to_string())),
                    (_, Some(v)) => Some((*field, *label, v.to_string())),
                    (_, None) => None,
                }
            })
            .collect();
        out.insert(
            id.to_string(),
            DecisionEntry {
                title,
                status,
                prose,
            },
        );
    }
    Ok(out)
}

pub(crate) fn read_open_questions(
    store: &EntryStore,
) -> Result<std::collections::BTreeMap<String, OpenQuestionEntry>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, OpenQuestionEntry> = BTreeMap::new();
    for entry in store.load(EntryKind::OpenQuestion)? {
        let item = &entry.value;
        let Some(id) = item.get("id").and_then(toml::Value::as_str) else {
            continue;
        };
        let status = item
            .get("status")
            .and_then(toml::Value::as_str)
            .unwrap_or("")
            .to_string();
        let affects: Vec<String> = item
            .get("affects")
            .and_then(toml::Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.insert(id.to_string(), OpenQuestionEntry { status, affects });
    }
    Ok(out)
}

fn read_requirement_decided_by(
    store: &EntryStore,
) -> Result<std::collections::BTreeMap<String, Vec<String>>> {
    use std::collections::BTreeMap;
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in store.load(EntryKind::Requirement)? {
        // A retired requirement (D117) pins nothing for live work.
        if crate::entries::is_retired_file(&entry.file) {
            continue;
        }
        let item = &entry.value;
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
