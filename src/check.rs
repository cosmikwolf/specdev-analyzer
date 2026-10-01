//! `specdev-analyzer check` — the spec checked against the code.
//!
//! `check` runs `validate` and then the checks that read the source tree:
//!
//! * **source rule** (`source_resolves`) — a module that is not `planned`
//!   and has no resolvable source is an error;
//! * **stale flags** (`stale_planned`) — a `planned` module whose source
//!   resolves, and a `planned` requirement whose satisfying modules are all
//!   built, are warnings;
//! * **open-question gate** (`implementation_unblocked`) — a module that an
//!   `open` question reaches (local or workspace tier) and whose source
//!   resolves is an error, planned or not: the gate keys on code existing;
//! * **`--since <ref>`** — every new `planned` / `test_pending` flag, every
//!   `hard_gate` flip, and every default committed on a `hard_gate = false`
//!   question between `<ref>` and the working tree (info findings).
//!
//! The principle every check follows: `planned` and `test_pending`
//! suppress only findings about something missing; code that exists is
//! always checked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::domain::{DomainConfig, Severity};
use crate::items::source_map::{resolve_source_path, source_file_override};
use crate::layout::LayoutContext;
use crate::report::{Finding, Report};

/// The gate's rule name when `domain.toml` declares none.
const GATE_RULE: &str = "implementation_unblocked";

/// Options for one `check` run.
#[derive(Debug, Default)]
pub struct CheckOptions {
    /// Git ref to diff the spec against (`--since`).
    pub since: Option<String>,
}

/// One spec module, as `check` sees it.
#[derive(Debug)]
struct ModuleState {
    name: String,
    file: PathBuf,
    planned: bool,
    source: Option<PathBuf>,
}

/// Run `validate` plus every check against the code.
pub fn run(layout: &LayoutContext, opts: &CheckOptions) -> Result<Report> {
    let mut report = crate::validate::run(layout)?;
    let domain = load_domain(layout);
    let modules = module_states(layout)?;

    source_rule(&modules, &domain, &mut report);
    stale_planned_requirements(layout, &modules, &domain, &mut report)?;
    open_question_gate(layout, &modules, &domain, &mut report)?;
    if let Some(since) = &opts.since {
        since_report(layout, since, &domain, &mut report)?;
    }
    Ok(report)
}

fn load_domain(layout: &LayoutContext) -> Option<DomainConfig> {
    DomainConfig::load(&layout.domain_toml_path).ok()
}

/// The severity `domain.toml` sets for `rule`, else `default`.
fn severity(domain: &Option<DomainConfig>, rule: &str, default: Severity) -> Severity {
    domain
        .as_ref()
        .and_then(|d| d.validation.rules.get(rule).copied())
        .unwrap_or(default)
}

fn rel(layout: &LayoutContext, p: &Path) -> String {
    p.strip_prefix(&layout.project_root)
        .unwrap_or(p)
        .display()
        .to_string()
}

fn module_states(layout: &LayoutContext) -> Result<Vec<ModuleState>> {
    let adapter = crate::lang::adapter_for(&layout.spec.project.language);
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&layout.modules_dir) else {
        return Ok(out);
    };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    files.sort();
    for file in files {
        let text = std::fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let Ok(doc) = text.parse::<toml::Table>() else {
            continue; // a parse error is already validate's finding
        };
        let doc = toml::Value::Table(doc);
        let name = doc
            .get("name")
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .or_else(|| file.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .unwrap_or_default();
        let planned = doc.get("planned").and_then(toml::Value::as_bool) == Some(true);
        let source = adapter.as_deref().and_then(|a| {
            resolve_source_path(
                &name,
                source_file_override(&doc),
                a,
                &layout.project_root,
                &layout.spec.paths.source,
            )
        });
        out.push(ModuleState { name, file, planned, source });
    }
    Ok(out)
}

fn source_rule(modules: &[ModuleState], domain: &Option<DomainConfig>, report: &mut Report) {
    let src_sev = severity(domain, "source_resolves", Severity::Error);
    let stale_sev = severity(domain, "stale_planned", Severity::Warning);
    for m in modules {
        match (m.planned, &m.source) {
            (false, None) => report.push(
                Finding::new(
                    "source_resolves",
                    src_sev,
                    format!(
                        "module `{}` has no resolvable source: set `source_file`, or `planned = true` \
                         while it is not built",
                        m.name
                    ),
                )
                .with_file(m.file.display().to_string())
                .with_entity(&m.name),
            ),
            (true, Some(src)) => report.push(
                Finding::new(
                    "stale_planned",
                    stale_sev,
                    format!(
                        "module `{}` is planned, but its source exists ({}); remove `planned`",
                        m.name,
                        src.display()
                    ),
                )
                .with_file(m.file.display().to_string())
                .with_entity(&m.name),
            ),
            _ => {}
        }
    }
}

/// A `planned` requirement whose satisfying modules are all built.
fn stale_planned_requirements(
    layout: &LayoutContext,
    modules: &[ModuleState],
    domain: &Option<DomainConfig>,
    report: &mut Report,
) -> Result<()> {
    let built: BTreeSet<&str> = modules
        .iter()
        .filter(|m| !m.planned && m.source.is_some())
        .map(|m| m.name.as_str())
        .collect();
    let module_names: BTreeSet<&str> = modules.iter().map(|m| m.name.as_str()).collect();
    let inv = crate::reductions::load_all(layout)?;
    let (kindset, _) = crate::reductions::resolve(&inv);
    let mut scratch = Report::default();
    let graph = crate::reduction::execute_kindset(&kindset, layout, &mut scratch)?;
    let graph = serde_json::to_value(&graph)?;
    let mut satisfiers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in graph.get("relations").and_then(Value::as_array).into_iter().flatten() {
        let (Some(from), Some(to)) = (
            r.get("from").and_then(Value::as_str),
            r.get("to").and_then(Value::as_str),
        ) else {
            continue;
        };
        if module_names.contains(from) {
            satisfiers.entry(to.to_string()).or_default().push(from.to_string());
        }
    }
    let sev = severity(domain, "stale_planned", Severity::Warning);
    for e in graph.get("entities").and_then(Value::as_array).into_iter().flatten() {
        let kind = e.get("kind").and_then(Value::as_str).unwrap_or("");
        if !(kind == "Requirement" || kind.ends_with(".Requirement")) {
            continue;
        }
        let Some(id) = e.get("name").and_then(Value::as_str) else { continue };
        let planned = e.get("attrs").and_then(|a| a.get("planned")).and_then(Value::as_bool) == Some(true);
        if !planned {
            continue;
        }
        let Some(sats) = satisfiers.get(id) else { continue };
        if !sats.is_empty() && sats.iter().all(|s| built.contains(s.as_str())) {
            report.push(
                Finding::new(
                    "stale_planned",
                    sev,
                    format!(
                        "requirement `{id}` is planned, but every module that satisfies it is built \
                         ({}); remove `planned`",
                        sats.join(", ")
                    ),
                )
                .with_entity(id),
            );
        }
    }
    Ok(())
}

/// The gate's rule name: the `module_reachable_through_open_question`
/// graph check `domain.toml` declares, else [`GATE_RULE`].
fn gate_rule(domain: &Option<DomainConfig>) -> String {
    domain
        .as_ref()
        .and_then(|d| {
            d.project
                .graph_checks
                .iter()
                .find(|c| c.kind == "module_reachable_through_open_question")
                .and_then(|c| c.rule.clone())
        })
        .unwrap_or_else(|| GATE_RULE.to_string())
}

fn open_question_gate(
    layout: &LayoutContext,
    modules: &[ModuleState],
    domain: &Option<DomainConfig>,
    report: &mut Report,
) -> Result<()> {
    let rule = gate_rule(domain);
    let sev = severity(domain, &rule, Severity::Error);
    for m in modules.iter().filter(|m| m.source.is_some()) {
        let expected = layout.modules_dir.join(format!("{}.toml", m.name));
        if !expected.is_file() {
            continue; // the gate reads the module by name; a renamed file is validate's finding
        }
        if let Some(refusal) = crate::prompt::check_codegen_gate(layout, &m.name)? {
            report.push(
                Finding::new(&rule, sev, refusal.message.trim_end().to_string())
                    .with_file(m.file.display().to_string())
                    .with_entity(&m.name),
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// --since
// ---------------------------------------------------------------------------

/// The TOML bodies of every spec file under `specs/` at `rev`, keyed by
/// project-relative path. `None` for `rev = None` means the working tree.
fn spec_files(layout: &LayoutContext, rev: Option<&str>) -> Result<BTreeMap<String, toml::Value>> {
    let specs_rel = rel(layout, &layout.specs_dir);
    let mut out = BTreeMap::new();
    let paths: Vec<String> = match rev {
        None => walk_toml(&layout.specs_dir)
            .into_iter()
            .map(|p| rel(layout, &p))
            .collect(),
        Some(rev) => {
            let ls = git(layout, &["ls-tree", "-r", "--name-only", rev, "--", &specs_rel])?;
            ls.lines().filter(|l| l.ends_with(".toml")).map(str::to_string).collect()
        }
    };
    for p in paths {
        if p.contains("/specdev/") {
            continue; // machinery, not spec content
        }
        let text = match rev {
            None => std::fs::read_to_string(layout.project_root.join(&p)).unwrap_or_default(),
            Some(rev) => git(layout, &["show", &format!("{rev}:./{p}")]).unwrap_or_default(),
        };
        if let Ok(t) = text.parse::<toml::Table>() {
            out.insert(p, toml::Value::Table(t));
        }
    }
    Ok(out)
}

fn walk_toml(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk_toml(&p));
            } else if p.extension().is_some_and(|x| x == "toml") {
                out.push(p);
            }
        }
    }
    out
}

fn git(layout: &LayoutContext, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(&layout.project_root)
        .output()
        .context("running git (`check --since` reads history)")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every flaggable entry in a set of spec files: module roots by `name`,
/// requirements and open questions by `id`.
#[derive(Debug, Default, Clone)]
struct Flags {
    planned: bool,
    test_pending: bool,
    hard_gate: bool,
    status: String,
}

fn flags_of(files: &BTreeMap<String, toml::Value>) -> BTreeMap<String, Flags> {
    let read = |v: &toml::Value| Flags {
        planned: v.get("planned").and_then(toml::Value::as_bool) == Some(true),
        test_pending: v.get("test_pending").and_then(toml::Value::as_bool) == Some(true),
        hard_gate: v.get("hard_gate").and_then(toml::Value::as_bool).unwrap_or(true),
        status: v.get("status").and_then(toml::Value::as_str).unwrap_or("").to_string(),
    };
    let mut out = BTreeMap::new();
    for (path, doc) in files {
        if path.contains("/modules/") {
            if let Some(name) = doc.get("name").and_then(toml::Value::as_str) {
                out.insert(name.to_string(), read(doc));
            }
            continue;
        }
        for table in ["requirement", "open_question"] {
            for entry in doc.get(table).and_then(toml::Value::as_array).into_iter().flatten() {
                if let Some(id) = entry.get("id").and_then(toml::Value::as_str) {
                    out.insert(id.to_string(), read(entry));
                }
            }
        }
    }
    out
}

fn since_report(
    layout: &LayoutContext,
    since: &str,
    domain: &Option<DomainConfig>,
    report: &mut Report,
) -> Result<()> {
    let before = flags_of(&spec_files(layout, Some(since))?);
    let after = flags_of(&spec_files(layout, None)?);
    let info = |rule: &str| severity(domain, rule, Severity::Info);
    for (key, now) in &after {
        let was = before.get(key).cloned().unwrap_or(Flags { hard_gate: true, ..Default::default() });
        for (flag, set_now, set_before) in [
            ("planned", now.planned, was.planned),
            ("test_pending", now.test_pending, was.test_pending),
        ] {
            if set_now && !set_before {
                report.push(
                    Finding::new(
                        "since_new_flag",
                        info("since_new_flag"),
                        format!("`{key}`: new `{flag} = true` since {since}"),
                    )
                    .with_entity(key),
                );
            }
        }
        if before.contains_key(key) && now.hard_gate != was.hard_gate {
            report.push(
                Finding::new(
                    "since_hard_gate_flip",
                    info("since_hard_gate_flip"),
                    format!("`{key}`: `hard_gate` {} → {} since {since}", was.hard_gate, now.hard_gate),
                )
                .with_entity(key),
            );
        }
        if !now.hard_gate && now.status == "default_committed" && was.status != "default_committed" {
            report.push(
                Finding::new(
                    "since_default_committed",
                    info("since_default_committed"),
                    format!("`{key}`: a default was committed on a `hard_gate = false` question since {since}"),
                )
                .with_entity(key),
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

/// The `--format yaml` report: `findings` plus `counts`.
pub fn to_yaml(report: &Report) -> Result<String> {
    let sev = |s: Severity| match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    };
    let findings: Vec<Value> = report
        .findings()
        .iter()
        .map(|f| {
            serde_json::json!({
                "rule": f.rule,
                "severity": sev(f.severity),
                "entity": f.entity,
                "file": f.file,
                "pointer": f.pointer,
                "line": f.line,
                "message": f.message,
            })
        })
        .collect();
    let (e, w, i) = report.counts();
    let doc = serde_json::json!({
        "findings": findings,
        "counts": { "error": e, "warning": w, "info": i },
    });
    Ok(serde_yaml::to_string(&doc)?)
}
