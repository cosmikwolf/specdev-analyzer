//! `report decisions` — which decision text reaches which module's prompt.
//!
//! A projection over [`prompt::decision_context`], the function the codegen
//! prompt itself is rendered from, so the report and the prompt cannot
//! disagree. Before it, the only way to answer "does this module's prompt
//! carry decision X, and which of its fields?" was to assemble every prompt
//! and search the text — which is how a stale accepted decision reaching one
//! module but not its sibling was found in the field (116 prompts, by hand).
//!
//! Two views of the same data:
//! - `modules`: per module, the decisions and open questions in its prompt,
//!   why each is there (`guided_by`, a satisfied requirement's `decided_by`,
//!   the workspace tier, an open question's `affects`), and which decision
//!   prose fields go in (`codegen_include`).
//! - `decisions`: per decision, the modules it reaches. A decision that
//!   reaches none is listed too — its text is in no prompt. Judging whether
//!   two decisions contradict each other is semantic and stays with a person; the
//!   report only puts the candidates side by side.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::layout::LayoutContext;
use crate::prompt::{self, ContextReason};

#[derive(Debug, Serialize)]
pub struct DecisionReport {
    pub modules: Vec<ModuleRow>,
    pub decisions: Vec<DecisionRow>,
}

#[derive(Debug, Serialize)]
pub struct ModuleRow {
    pub module: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<ModuleDecision>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub open_questions: Vec<ModuleQuestion>,
}

#[derive(Debug, Serialize)]
pub struct ModuleDecision {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub workspace: bool,
    /// The prose fields that reach the prompt, in render order.
    pub fields: Vec<&'static str>,
    pub why: Vec<ContextReason>,
}

#[derive(Debug, Serialize)]
pub struct ModuleQuestion {
    pub id: String,
    pub status: String,
    pub why: Vec<ContextReason>,
}

#[derive(Debug, Serialize)]
pub struct DecisionRow {
    pub id: String,
    pub title: String,
    pub status: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub workspace: bool,
    /// Modules whose prompt carries this decision. Empty for a superseded
    /// decision, and for one nothing links to.
    pub reaches: Vec<String>,
}

/// Build the report. `module` limits the `modules` view to one module;
/// `decision` limits the `decisions` view to one id. Each filter must name
/// something that exists.
pub fn build(
    layout: &LayoutContext,
    module: Option<&str>,
    decision: Option<&str>,
) -> Result<DecisionReport> {
    let names = prompt::list_module_names(layout)?;
    if let Some(m) = module {
        if !names.iter().any(|n| n == m) {
            bail!("--module `{m}`: no specs/modules/{m}.toml");
        }
    }

    let mut modules = Vec::new();
    let mut reaches: BTreeMap<(bool, String), Vec<String>> = BTreeMap::new();
    for name in &names {
        let path = layout.modules_dir.join(format!("{name}.toml"));
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let toml: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let ctx = prompt::decision_context(layout, &toml)?;
        for d in &ctx.decisions {
            reaches
                .entry((d.workspace, d.id.clone()))
                .or_default()
                .push(name.clone());
        }
        if module.is_some_and(|m| m != name) {
            continue;
        }
        modules.push(ModuleRow {
            module: name.clone(),
            decisions: ctx
                .decisions
                .into_iter()
                .map(|d| ModuleDecision {
                    id: d.id,
                    title: d.title,
                    workspace: d.workspace,
                    fields: d.fields,
                    why: d.reasons,
                })
                .collect(),
            open_questions: ctx
                .open_questions
                .into_iter()
                .map(|q| ModuleQuestion {
                    id: q.id,
                    status: q.status,
                    why: q.reasons,
                })
                .collect(),
        });
    }

    let all = prompt::project_decisions(layout)?;
    if let Some(d) = decision {
        if !all.iter().any(|(id, ..)| id == d) {
            bail!("--decision `{d}`: no decision with that id");
        }
    }
    let decisions = all
        .into_iter()
        .filter(|(id, ..)| decision.is_none_or(|d| d == id))
        .map(|(id, title, status, workspace)| DecisionRow {
            reaches: reaches.remove(&(workspace, id.clone())).unwrap_or_default(),
            id,
            title,
            status,
            workspace,
        })
        .collect();

    Ok(DecisionReport { modules, decisions })
}

pub fn render_yaml(report: &DecisionReport) -> Result<String> {
    serde_yaml::to_string(report).context("serializing the decision report")
}
