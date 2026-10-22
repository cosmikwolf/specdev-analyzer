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
//! * **rule packs** (`pack_entity_in_code`, `pack_attribute_matches`) —
//!   each inlined pack's code checks: see [`crate::code_checks`];
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
    depends_on: Vec<String>,
    /// The names the spec gives the module's parts (see [`crate::source_hint`]).
    names: Vec<String>,
}

/// Run `validate` plus every check against the code.
pub fn run(layout: &LayoutContext, opts: &CheckOptions) -> Result<Report> {
    let mut report = crate::validate::run(layout)?;
    let domain = load_domain(layout);
    let modules = module_states(layout)?;

    let graph = project_graph(layout)?;
    let names: Vec<&str> = modules.iter().map(|m| m.name.as_str()).collect();
    let with_entries = crate::code_checks::modules_with_checked_entries(layout, &graph, &names);
    source_rule(layout, &modules, &with_entries, &domain, &mut report);
    stale_planned_requirements(&graph, &modules, &domain, &mut report);
    open_question_gate(layout, &modules, &domain, &mut report)?;
    test_checks(layout, &graph, &modules, &domain, &mut report)?;
    internal_edges(layout, &modules, &domain, &mut report)?;
    pack_code_checks(layout, &graph, &modules, &domain, &mut report);
    unreached_decisions(layout, &domain, &mut report);
    toolchain_rule(layout, &domain, &mut report);
    if let Some(since) = &opts.since {
        since_report(layout, since, &domain, &mut report)?;
    }
    Ok(report)
}

/// An accepted decision of this project that reaches no module guides no
/// code: the agent that follows `report decisions` never sees it. Reported
/// as info, with the three ways to link it. A workspace root is skipped:
/// its decisions reach the members' modules, which its own report does not
/// follow.
fn unreached_decisions(layout: &LayoutContext, domain: &Option<DomainConfig>, report: &mut Report) {
    if layout.spec.workspace.is_some() {
        return;
    }
    let Ok(decisions) = crate::decision_report::build(layout, None, None) else {
        return;
    };
    let sev = severity(domain, "decision_reaches_nothing", Severity::Info);
    let store = crate::entries::EntryStore::for_layout(layout);
    for d in decisions.decisions {
        if d.workspace || d.status != "accepted" || !d.reaches.is_empty() {
            continue;
        }
        report.push(
            Finding::new(
                "decision_reaches_nothing",
                sev,
                format!(
                    "decision `{}` is accepted but reaches no module, so it guides no code: name it in a \
                     module's `guided_by`, or in the `decided_by` of a requirement a module satisfies",
                    d.id
                ),
            )
            .with_file(
                store
                    .location_hint(crate::entries::EntryKind::Decision, &d.id)
                    .trim_matches('`')
                    .to_string(),
            )
            .with_entity(&d.id),
        );
    }
}

/// The spec's `[project] toolchain` against the profile's `toolchains:` and
/// the tree: the tool must be one the profile lists, and no other listed
/// tool's lockfile may be present. A missing lockfile of the declared tool
/// is only info (a fresh tree has none before the first install).
fn toolchain_rule(layout: &LayoutContext, domain: &Option<DomainConfig>, report: &mut Report) {
    let Ok(Some(profile)) = crate::manifest::Profile::load_inlined(&layout.specdev_root) else {
        return;
    };
    if profile.toolchains.is_empty() {
        return;
    }
    let spec_file = layout.spec_toml_path.display().to_string();
    let known: Vec<&str> = profile.toolchains.keys().map(String::as_str).collect();
    let declared = layout.spec.project.toolchain.as_deref();
    let Some(tool) = declared.filter(|t| profile.toolchains.contains_key(*t)) else {
        let (sev, msg) = match declared {
            None => (
                severity(domain, "toolchain_declared", Severity::Warning),
                format!(
                    "profile `{}` gives gates per tool ({}): declare the project's tool as \
                     `[project] toolchain = \"<tool>\"`",
                    profile.name,
                    known.join(", ")
                ),
            ),
            Some(t) => (
                Severity::Error,
                format!(
                    "`toolchain = \"{t}\"` is not a tool profile `{}` supports ({})",
                    profile.name,
                    known.join(", ")
                ),
            ),
        };
        report.push(Finding::new("toolchain_declared", sev, msg).with_file(spec_file));
        return;
    };
    let mine = &profile.toolchains[tool].lockfiles;
    let sev = severity(domain, "toolchain_matches_tree", Severity::Warning);
    for (other, chain) in &profile.toolchains {
        for lock in &chain.lockfiles {
            if other != tool && !mine.contains(lock) && layout.project_root.join(lock).is_file() {
                report.push(
                    Finding::new(
                        "toolchain_matches_tree",
                        sev,
                        format!(
                            "`{lock}` is a `{other}` lockfile, but spec.toml declares \
                             `toolchain = \"{tool}\"`: remove it, or change the declared tool"
                        ),
                    )
                    .with_file(lock.clone()),
                );
            }
        }
    }
    if !mine.is_empty() && !mine.iter().any(|l| layout.project_root.join(l).is_file()) {
        report.push(
            Finding::new(
                "toolchain_matches_tree",
                Severity::Info,
                format!(
                    "no {} lockfile yet ({}); create it with the tool and commit it",
                    tool,
                    mine.join(", ")
                ),
            )
            .with_file(spec_file),
        );
    }
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
        let text = std::fs::read_to_string(&file)
            .with_context(|| format!("reading {}", file.display()))?;
        let Ok(doc) = text.parse::<toml::Table>() else {
            continue; // a parse error is already validate's finding
        };
        let doc = toml::Value::Table(doc);
        let name = doc
            .get("name")
            .and_then(toml::Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                file.file_stem()
                    .and_then(|s| s.to_str())
                    .map(str::to_string)
            })
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
        let depends_on = string_list(doc.get("depends_on"));
        let names = crate::source_hint::module_names(&doc);
        out.push(ModuleState {
            name,
            file,
            planned,
            source,
            depends_on,
            names,
        });
    }
    Ok(out)
}

fn source_rule(
    layout: &LayoutContext,
    modules: &[ModuleState],
    with_entries: &BTreeSet<String>,
    domain: &Option<DomainConfig>,
    report: &mut Report,
) {
    let src_sev = severity(domain, "source_resolves", Severity::Error);
    let stale_sev = severity(domain, "stale_planned", Severity::Warning);
    // Indexed only when some module needs it: one parse of each source file.
    let index = modules
        .iter()
        .any(|m| !m.planned && m.source.is_none() && !m.names.is_empty())
        .then(|| crate::lang::adapter_for(&layout.spec.project.language))
        .flatten()
        .map(|a| {
            crate::source_hint::NameIndex::build(
                &layout.project_root,
                &layout.project_root.join(&layout.spec.paths.source),
                a.as_ref(),
            )
        });
    for m in modules {
        match (m.planned, &m.source) {
            (false, None) => report.push(
                Finding::new(
                    "source_resolves",
                    src_sev,
                    format!(
                        "module `{}` has no resolvable source: set `source_file`, or `planned = true` \
                         while it is not built{}",
                        m.name,
                        index
                            .as_ref()
                            .and_then(|i| crate::source_hint::render(
                                &i.candidates(&m.names),
                                m.names.len()
                            ))
                            .map(|h| format!("; {h}"))
                            .unwrap_or_default()
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
                        "module `{}` is planned, but its source exists ({}); remove `planned`{}",
                        m.name,
                        src.display(),
                        if with_entries.contains(&m.name) {
                            "; if some of its entries are not built yet, put `planned = true` on \
                             those entries instead"
                        } else {
                            ""
                        }
                    ),
                )
                .with_file(m.file.display().to_string())
                .with_entity(&m.name),
            ),
            _ => {}
        }
    }
}

/// Each inlined rule pack's spec ↔ code checks.
fn pack_code_checks(
    layout: &LayoutContext,
    graph: &Value,
    modules: &[ModuleState],
    domain: &Option<DomainConfig>,
    report: &mut Report,
) {
    let views: Vec<crate::code_checks::ModuleView<'_>> = modules
        .iter()
        .map(|m| crate::code_checks::ModuleView {
            name: &m.name,
            planned: m.planned,
            built: m.source.is_some(),
        })
        .collect();
    let empty = BTreeMap::new();
    let rules = domain
        .as_ref()
        .map(|d| &d.validation.rules)
        .unwrap_or(&empty);
    crate::code_checks::run(layout, graph, &views, rules, report);
}

/// The reduced project graph as JSON.
fn project_graph(layout: &LayoutContext) -> Result<Value> {
    let inv = crate::reductions::load_all(layout)?;
    let (kindset, _) = crate::reductions::resolve(&inv);
    let mut scratch = Report::default();
    let graph = crate::reduction::execute_kindset(&kindset, layout, &mut scratch)?;
    Ok(serde_json::to_value(&graph)?)
}

/// Requirement id → the spec modules that satisfy it.
fn satisfiers_of(graph: &Value, modules: &[ModuleState]) -> BTreeMap<String, Vec<String>> {
    let module_names: BTreeSet<&str> = modules.iter().map(|m| m.name.as_str()).collect();
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for r in graph
        .get("relations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(from), Some(to)) = (
            r.get("from").and_then(Value::as_str),
            r.get("to").and_then(Value::as_str),
        ) else {
            continue;
        };
        if module_names.contains(from) {
            out.entry(to.to_string())
                .or_default()
                .push(from.to_string());
        }
    }
    out
}

fn is_requirement_kind(kind: &str) -> bool {
    kind == "Requirement" || kind.ends_with(".Requirement")
}

/// A `planned` requirement whose satisfying modules are all built.
fn stale_planned_requirements(
    graph: &Value,
    modules: &[ModuleState],
    domain: &Option<DomainConfig>,
    report: &mut Report,
) {
    let built: BTreeSet<&str> = modules
        .iter()
        .filter(|m| !m.planned && m.source.is_some())
        .map(|m| m.name.as_str())
        .collect();
    let satisfiers = satisfiers_of(graph, modules);
    let sev = severity(domain, "stale_planned", Severity::Warning);
    for e in graph
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = e.get("kind").and_then(Value::as_str).unwrap_or("");
        if !is_requirement_kind(kind) {
            continue;
        }
        let Some(id) = e.get("name").and_then(Value::as_str) else {
            continue;
        };
        let flag = |k: &str| {
            e.get("attrs")
                .and_then(|a| a.get(k))
                .and_then(Value::as_bool)
                == Some(true)
        };
        // A retired requirement is exempt from the stale flags.
        if !flag("planned") || flag("retired") {
            continue;
        }
        let Some(sats) = satisfiers.get(id) else {
            continue;
        };
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
}

/// The rule name a `domain.toml` graph check of `kind` declares, else `default`.
fn declared_rule(domain: &Option<DomainConfig>, kind: &str, default: &str) -> String {
    domain
        .as_ref()
        .and_then(|d| {
            d.project
                .graph_checks
                .iter()
                .find(|c| c.kind == kind)
                .and_then(|c| c.rule.clone())
        })
        .unwrap_or_else(|| default.to_string())
}

fn string_list(v: Option<&toml::Value>) -> Vec<String> {
    v.and_then(toml::Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Requirement ↔ test, on the test-discovery pass:
///
/// * `requirements_have_tests` — a `nature = "module"` or `"test"`
///   requirement that is not planned (set, or derived: ≥1 satisfier, all
///   planned) and not `test_pending` has ≥1 `verified_by` entry;
/// * `verified_by_resolves` — every `verified_by` name, on every entity kind
///   that carries one, names a discovered test or a container of tests
///   (a missing name is suppressed under `planned` / `test_pending`);
/// * `verified_only_by_ignored_tests` — every test a requirement names is
///   switched off: reported pending, not passed;
/// * `stale_test_pending` — `test_pending` while every name resolves.
fn test_checks(
    layout: &LayoutContext,
    graph: &Value,
    modules: &[ModuleState],
    domain: &Option<DomainConfig>,
    report: &mut Report,
) -> Result<()> {
    let disc = crate::test_discovery::discover_project(layout)?;
    if !disc.checked {
        return Ok(()); // the language ships no tests.scm: nothing to check against
    }
    let has_test = declared_rule(domain, "requirements_have_tests", "requirements_have_tests");
    let resolves = declared_rule(domain, "verified_by_resolves", "verified_by_resolves");
    let sev_has = severity(domain, &has_test, Severity::Error);
    let sev_res = severity(domain, &resolves, Severity::Error);
    let sev_ignored = severity(domain, "verified_only_by_ignored_tests", Severity::Info);
    let sev_stale = severity(domain, "stale_test_pending", Severity::Warning);
    let planned_module: BTreeMap<&str, bool> = modules
        .iter()
        .map(|m| (m.name.as_str(), m.planned))
        .collect();
    let satisfiers = satisfiers_of(graph, modules);

    let store = crate::entries::EntryStore::for_layout(layout);
    let requirements = store
        .load(crate::entries::EntryKind::Requirement)
        .unwrap_or_default();
    // A parent is covered by its children: each child carries "has a test".
    let parents = crate::requirement_tree::Tree::load(&store).exempt();
    for entry in requirements {
        let v = &entry.value;
        let Some(id) = v.get("id").and_then(toml::Value::as_str) else {
            continue;
        };
        let nature = v
            .get("nature")
            .and_then(toml::Value::as_str)
            .unwrap_or("module");
        let planned_set = v.get("planned").and_then(toml::Value::as_bool) == Some(true);
        let derived = satisfiers.get(id).is_some_and(|s| {
            !s.is_empty()
                && s.iter()
                    .all(|m| planned_module.get(m.as_str()).copied().unwrap_or(false))
        });
        let planned = planned_set || derived;
        let pending_flag = v.get("test_pending").and_then(toml::Value::as_bool) == Some(true);
        let names = string_list(v.get("verified_by"));
        let file = entry.file.display().to_string();

        // A retired requirement is history, exempt from every
        // requirement ↔ test check. A test it still names that exists is
        // code that exists, so that is reported, not hidden: code that
        // exists is always checked.
        if crate::entries::is_retired_file(&entry.file) {
            let live: Vec<&str> = names
                .iter()
                .filter(|n| !disc.resolve(n).is_empty())
                .map(String::as_str)
                .collect();
            if !live.is_empty() {
                report.push(
                    Finding::new(
                        crate::retired::TESTED,
                        severity(domain, crate::retired::TESTED, Severity::Warning),
                        format!(
                            "retired requirement `{id}` still names tests that exist ({}): remove them,                              or drop them from `verified_by`",
                            live.join(", ")
                        ),
                    )
                    .with_file(&file)
                    .with_entity(id),
                );
            }
            continue;
        }

        if matches!(nature, "module" | "test")
            && !planned
            && !pending_flag
            && names.is_empty()
            && !parents.contains(id)
        {
            report.push(
                Finding::new(
                    &has_test,
                    sev_has,
                    format!(
                        "requirement `{id}` has no `verified_by` test; name one, or set \
                         `test_pending = true` while it has none"
                    ),
                )
                .with_file(&file)
                .with_entity(id),
            );
        }
        let resolved: Vec<(String, Vec<&crate::test_discovery::DiscoveredTest>)> =
            names.iter().map(|n| (n.clone(), disc.resolve(n))).collect();
        if !planned && !pending_flag {
            for (n, hits) in &resolved {
                if hits.is_empty() {
                    report.push(
                        Finding::new(&resolves, sev_res, disc.unresolved_message(id, n))
                            .with_file(&file)
                            .with_entity(id),
                    );
                }
            }
        }
        let all_resolve = !resolved.is_empty() && resolved.iter().all(|(_, h)| !h.is_empty());
        if all_resolve
            && !planned
            && !pending_flag
            && resolved.iter().all(|(_, h)| h.iter().all(|t| t.pending))
        {
            report.push(
                Finding::new(
                    "verified_only_by_ignored_tests",
                    sev_ignored,
                    format!("requirement `{id}` is verified only by switched-off tests: pending"),
                )
                .with_file(&file)
                .with_entity(id),
            );
        }
        if pending_flag && all_resolve {
            report.push(
                Finding::new(
                    "stale_test_pending",
                    sev_stale,
                    format!("requirement `{id}` is `test_pending`, but every test it names exists; remove the flag"),
                )
                .with_file(&file)
                .with_entity(id),
            );
        }
    }

    // Every other entity kind that carries `verified_by`.
    for e in graph
        .get("entities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = e.get("kind").and_then(Value::as_str).unwrap_or("");
        if is_requirement_kind(kind) {
            continue;
        }
        let Some(name) = e.get("name").and_then(Value::as_str) else {
            continue;
        };
        let attrs = e.get("attrs");
        if attrs
            .and_then(|a| a.get("planned"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let names: Vec<&str> = attrs
            .and_then(|a| a.get("verified_by"))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for n in names {
            if disc.resolve(n).is_empty() {
                report.push(
                    Finding::new(&resolves, sev_res, disc.unresolved_message(name, n))
                        .with_entity(name),
                );
            }
        }
    }
    Ok(())
}

/// The shortest `depends_on` path from `start` to `goal`, both included,
/// or `None` when `goal` is not reachable.
fn declared_path(
    declared: &BTreeMap<&str, &[String]>,
    start: &str,
    goal: &str,
) -> Option<Vec<String>> {
    let mut parent: BTreeMap<String, String> = BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([start.to_string()]);
    let mut seen = std::collections::BTreeSet::from([start.to_string()]);
    while let Some(node) = queue.pop_front() {
        if node == goal {
            let mut path = vec![node.clone()];
            let mut cur = node;
            while let Some(p) = parent.get(&cur) {
                path.push(p.clone());
                cur = p.clone();
            }
            path.reverse();
            return Some(path);
        }
        for next in declared.get(node.as_str()).copied().unwrap_or_default() {
            if seen.insert(next.clone()) {
                parent.insert(next.clone(), node.clone());
                queue.push_back(next.clone());
            }
        }
    }
    None
}

/// Internal edges: an import from one module's source to another module
/// must be in the importing module's `depends_on`. Modules that share one
/// source file are one node, and each is told its edges are not checked.
fn internal_edges(
    layout: &LayoutContext,
    modules: &[ModuleState],
    domain: &Option<DomainConfig>,
    report: &mut Report,
) -> Result<()> {
    let nodes: Vec<crate::import_edges::ModuleNode> = modules
        .iter()
        .map(|m| crate::import_edges::ModuleNode {
            name: m.name.clone(),
            source: m.source.as_ref().map(|s| rel(layout, s)),
            depends_on: m.depends_on.clone(),
        })
        .collect();
    let file_of: BTreeMap<&str, String> = modules
        .iter()
        .map(|m| (m.name.as_str(), m.file.display().to_string()))
        .collect();
    let edges = crate::import_edges::module_edges(layout, &nodes)?;
    let declared: BTreeMap<&str, &[String]> = modules
        .iter()
        .map(|m| (m.name.as_str(), m.depends_on.as_slice()))
        .collect();
    let sev = severity(domain, "internal_edges_declared", Severity::Error);
    for e in edges.undeclared {
        let mut message = format!(
            "module `{}` imports module `{}` (`{}` in {}), which is not in its `depends_on`",
            e.from, e.to, e.spec, e.file
        );
        // #245: when the reverse path is declared, the obvious fix closes
        // a cycle, so name the path and the exits instead.
        if let Some(path) = declared_path(&declared, &e.to, &e.from) {
            message.push_str(&format!(
                "; adding it closes a cycle with the declared path {}: move the used item into `{}`, or invert the use",
                path.join(" -> "),
                e.from
            ));
        }
        report.push(
            Finding::new("internal_edges_declared", sev, message)
                .with_file(file_of.get(e.from.as_str()).cloned().unwrap_or_default())
                .with_entity(&e.from),
        );
    }
    let sev = severity(domain, "internal_edges_skipped", Severity::Info);
    for (module, file, others) in edges.shared {
        report.push(
            Finding::new(
                "internal_edges_skipped",
                sev,
                format!(
                    "module `{module}` shares {file} with {}: they are one node, so the edges between them are not checked",
                    others.join(", ")
                ),
            )
            .with_file(file_of.get(module.as_str()).cloned().unwrap_or_default())
            .with_entity(&module),
        );
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
            let ls = git(
                layout,
                &["ls-tree", "-r", "--name-only", rev, "--", &specs_rel],
            )?;
            ls.lines()
                .filter(|l| l.ends_with(".toml"))
                .map(str::to_string)
                .collect()
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
        hard_gate: v
            .get("hard_gate")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true),
        status: v
            .get("status")
            .and_then(toml::Value::as_str)
            .unwrap_or("")
            .to_string(),
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
            for entry in doc
                .get(table)
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = entry.get("id").and_then(toml::Value::as_str) {
                    out.insert(id.to_string(), read(entry));
                }
            }
        }
    }
    out
}

/// The control on retiring: each requirement retired since `since`, naming the
/// decision and saying when that decision is itself new in the range, and
/// each requirement id that was deleted rather than retired.
fn since_retirements(
    before: &BTreeMap<String, toml::Value>,
    after: &BTreeMap<String, toml::Value>,
    since: &str,
    info: &dyn Fn(&str) -> Severity,
    report: &mut Report,
) {
    // id → whether its file is a retired one, and its `retired_by`.
    let requirements = |files: &BTreeMap<String, toml::Value>| {
        let mut out: BTreeMap<String, (bool, Option<String>)> = BTreeMap::new();
        for (path, doc) in files {
            for e in doc
                .get("requirement")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(id) = e.get("id").and_then(toml::Value::as_str) {
                    let by = e
                        .get(crate::retired::FIELD)
                        .and_then(toml::Value::as_str)
                        .map(str::to_string);
                    out.insert(
                        id.to_string(),
                        (crate::entries::is_retired_file(Path::new(path)), by),
                    );
                }
            }
        }
        out
    };
    let decisions_before: BTreeSet<String> = before
        .values()
        .flat_map(|doc| {
            doc.get("decision")
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|d| {
                    d.get("id")
                        .and_then(toml::Value::as_str)
                        .map(str::to_string)
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let (was, now) = (requirements(before), requirements(after));
    for (id, (retired, by)) in &now {
        if *retired && !was.get(id).is_some_and(|(r, _)| *r) {
            let by = by.as_deref().unwrap_or("nothing");
            let new = if decisions_before.contains(by) {
                ""
            } else {
                " (a decision new in this range)"
            };
            report.push(
                Finding::new(
                    "since_retired",
                    info("since_retired"),
                    format!("`{id}`: retired by {by}{new} since {since}"),
                )
                .with_entity(id),
            );
        }
    }
    for id in was.keys().filter(|id| !now.contains_key(*id)) {
        report.push(
            Finding::new(
                "since_deleted_requirement",
                info("since_deleted_requirement"),
                format!(
                    "`{id}`: requirement deleted since {since}; to end a requirement, retire it \
                     (`specs/requirements/retired/` with `retired_by`) so its id stays taken"
                ),
            )
            .with_entity(id),
        );
    }
}

fn since_report(
    layout: &LayoutContext,
    since: &str,
    domain: &Option<DomainConfig>,
    report: &mut Report,
) -> Result<()> {
    let before_files = spec_files(layout, Some(since))?;
    let after_files = spec_files(layout, None)?;
    let before = flags_of(&before_files);
    let after = flags_of(&after_files);
    let info = |rule: &str| severity(domain, rule, Severity::Info);
    since_retirements(&before_files, &after_files, since, &info, report);
    for (key, now) in &after {
        let was = before.get(key).cloned().unwrap_or(Flags {
            hard_gate: true,
            ..Default::default()
        });
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
                    format!(
                        "`{key}`: `hard_gate` {} → {} since {since}",
                        was.hard_gate, now.hard_gate
                    ),
                )
                .with_entity(key),
            );
        }
        if !now.hard_gate && now.status == "default_committed" && was.status != "default_committed"
        {
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
/// [`to_yaml`] for a workspace root checked with its members: the root's
/// document, plus `members:`, one entry per member with its `path` and its
/// own `findings` and `counts` (`error: could not run` when it did not).
pub fn to_yaml_with_members(
    report: &Report,
    members: &[(String, Option<Report>)],
) -> Result<String> {
    let mut doc: Value = serde_yaml::from_str(&to_yaml(report)?)?;
    let list: Vec<Value> = members
        .iter()
        .map(|(path, r)| -> Result<Value> {
            Ok(match r {
                Some(r) => {
                    let mut m: Value = serde_yaml::from_str(&to_yaml(r)?)?;
                    m["path"] = Value::String(path.clone());
                    m
                }
                None => serde_json::json!({ "path": path, "error": "could not run" }),
            })
        })
        .collect::<Result<_>>()?;
    doc["members"] = Value::Array(list);
    Ok(serde_yaml::to_string(&doc)?)
}

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
