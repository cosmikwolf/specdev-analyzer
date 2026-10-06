//! Validate-pipeline orchestrator for the blocks + bridges + profiles arc.
//!
//! Layered:
//!   1. [`validate_layout`] — discovers `spec.toml`, builds a
//!      [`LayoutContext`], enumerates the `specs/specdev/` tree, runs the
//!      aggregator `$ref` resolver and reduction loader, and parses
//!      `domain.toml` for graph-check entries.
//!   2. [`validate_modules`] — compiles the aggregator schema, resolves the
//!      effective open-vocabulary kind set from the project's reductions,
//!      validates every module TOML under `specs/modules/` against the
//!      compiled aggregated schema, and resolves the kind references in
//!      every `domain.toml` graph-check entry against the effective set.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use jsonschema::{Retrieve, Uri};
use serde_json::Value as JsonValue;
use tracing::{info, instrument};

use crate::aggregator;
use crate::domain::{DomainConfig, GraphCheck, Severity};
use crate::entries::{EntryKind, EntryLayout, EntryStore};
use crate::graph as graph_checks;
use crate::kinds;
use crate::layout::LayoutContext;
use crate::reduction::{self, Graph};
use crate::reductions;
use crate::report::{Finding, Report};
use crate::resolved::{EffectiveKindSet, ResolvedExtends};

/// Legacy compatibility shim — old callers expected an `Inputs` struct. The
/// new shape is a [`LayoutContext`]; this alias keeps the public API surface
/// recognizable while we land the rewrite.
pub type Inputs = LayoutContext;

/// Library-friendly entry point. Discover `spec.toml` from `start` (or from
/// `explicit_spec_path` if set) and run validation end-to-end.
pub fn run_project(start: &Path, explicit_spec_path: Option<&Path>) -> Result<Report> {
    let layout = LayoutContext::discover(start, explicit_spec_path)?;
    run(&layout)
}

#[instrument(level = "info", skip(layout), fields(project = %layout.project_root.display()))]
pub fn run(layout: &LayoutContext) -> Result<Report> {
    let mut report = Report::default();
    let major = layout.spec.format_major();
    if major < crate::config::NEWEST_FORMAT_MAJOR {
        report.push(Finding::new(
            "FORMAT-001",
            Severity::Info,
            format!(
                "spec_version {} is an older project format; the newest this analyzer knows is {}.0. \
                 Run `specdev-analyzer migrate --dry-run` to see the steps, then `specdev-analyzer migrate`",
                layout.spec.spec_version,
                crate::config::NEWEST_FORMAT_MAJOR
            ),
        ));
    }
    validate_layout(layout, &mut report)?;
    validate_modules(layout, &mut report)?;
    validate_architecture(layout, &mut report)?;
    validate_entries(layout, &mut report)?;
    for f in crate::gates::script_findings(layout) {
        report.push(f);
    }
    Ok(report)
}

/// Schema-validate `specs/architecture.toml` against
/// `specs/specdev/schemas/architecture.schema.yaml`. Findings emitted via
/// `SCHEMA-ARCH`.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_architecture(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    validate_aux_toml(
        layout,
        &layout.architecture_schema_path,
        &layout.architecture_toml_path,
        "architecture",
        "SCHEMA-ARCH",
        Some("LAYOUT-005"),
        report,
    )
}

/// Schema-validate every entry file — decisions, requirements, sources and
/// open questions — against the profile's schema for its kind, one file at
/// a time. In the per-entry layout (`spec_version` `2.x`) also check that
/// each file holds exactly one entry of its kind and that its stem is the
/// entry's id.
///
/// Findings: `SCHEMA-REQ` (requirements and sources), `SCHEMA-DEC`,
/// `SCHEMA-OQ`, `ENTRY-FILE-001` (shape), `ENTRY-FILE-002` (stem),
/// `ENTRY-LEGACY-001` (a `1.x` file with content in a `2.x` project);
/// `LAYOUT-006` (info) when no requirement or source is authored.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_entries(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    let store = EntryStore::for_layout(layout);
    let per_entry = store.layout() == EntryLayout::PerEntry;

    for path in store.legacy_content() {
        report.push(
            Finding::new(
                "ENTRY-LEGACY-001",
                Severity::Error,
                EntryStore::legacy_message(&path),
            )
            .with_file(path.display().to_string()),
        );
    }

    // Requirements and sources share a schema and, in `1.x`, a file.
    let mut req_files = store.files(EntryKind::Requirement);
    for file in store.files(EntryKind::Source) {
        if !req_files.contains(&file) {
            req_files.push(file);
        }
    }
    if req_files.is_empty() && layout.requirements_schema_path.is_file() {
        let expected = store.expected_file(EntryKind::Requirement);
        report.push(Finding::new(
            "LAYOUT-006",
            Severity::Info,
            if per_entry {
                format!(
                    "no requirement or source entries under {} — none authored yet",
                    expected.display()
                )
            } else {
                format!(
                    "{} absent at {} — no requirements-level entries authored yet",
                    file_label(&expected),
                    expected.display()
                )
            },
        ));
    }
    for file in &req_files {
        validate_aux_toml(
            layout,
            &layout.requirements_schema_path,
            file,
            "requirements",
            "SCHEMA-REQ",
            None,
            report,
        )?;
    }

    for (kind, schema, rule) in [
        (EntryKind::Decision, "decisions.schema.yaml", "SCHEMA-DEC"),
        (
            EntryKind::OpenQuestion,
            "open_questions.schema.yaml",
            "SCHEMA-OQ",
        ),
    ] {
        let schema_path = layout.schemas_dir.join(schema);
        for file in store.files(kind) {
            validate_aux_toml(layout, &schema_path, &file, schema, rule, None, report)?;
        }
    }

    if per_entry {
        for kind in EntryKind::ALL {
            for file in store.files(kind) {
                check_entry_file(&file, kind, report);
            }
        }
    }
    Ok(())
}

/// `ENTRY-FILE-001` / `ENTRY-FILE-002`: a per-entry file holds exactly one
/// table of its kind and nothing else, and its stem is that entry's id. A
/// file that does not parse is left to the schema pass, which reports it.
fn check_entry_file(file: &Path, kind: EntryKind, report: &mut Report) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let Ok(parsed) = toml::from_str::<toml::Table>(&text) else {
        return;
    };
    let table = kind.table();
    let entries = parsed.get(table).and_then(toml::Value::as_array);
    let only_its_kind = parsed.keys().all(|k| k == table);
    let Some([entry]) = entries.map(Vec::as_slice).filter(|_| only_its_kind) else {
        report.push(
            Finding::new(
                "ENTRY-FILE-001",
                Severity::Error,
                format!(
                    "{} must hold exactly one [[{table}]] and nothing else — one entry per file",
                    file.display()
                ),
            )
            .with_file(file.display().to_string()),
        );
        return;
    };
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let id = entry.get("id").and_then(toml::Value::as_str).unwrap_or("");
    if stem != id {
        report.push(
            Finding::new(
                "ENTRY-FILE-002",
                Severity::Error,
                format!(
                    "{} holds `{id}`; the file name must be `{id}.toml` (the id names the file)",
                    file.display()
                ),
            )
            .with_file(file.display().to_string()),
        );
    }
}

/// Shared core for `architecture` + `requirements` schema validation.
/// Both files are profile-owned, optional in practice (a project may have
/// no architecture-level wiring or no requirements yet), and validated
/// against a sibling schema that may `$ref` other schemas in the same
/// directory (typically `enums.schema.yaml`).
fn validate_aux_toml(
    layout: &LayoutContext,
    schema_path: &Path,
    toml_path: &Path,
    label: &str,
    schema_rule: &str,
    absent_rule: Option<&str>,
    report: &mut Report,
) -> Result<()> {
    if !schema_path.is_file() {
        // Already flagged by `validate_layout`'s file_entries pass; skip
        // here so we don't double-report.
        return Ok(());
    }
    if !toml_path.is_file() {
        if let Some(absent_rule) = absent_rule {
            report.push(Finding::new(
                absent_rule,
                Severity::Info,
                format!(
                    "{label}.toml absent at {} — no {label}-level entries authored yet",
                    toml_path.display()
                ),
            ));
        }
        return Ok(());
    }

    let docs = build_aux_retriever_docs(layout)?;
    let retriever = AuxRetriever {
        docs: Arc::new(docs),
    };

    let schema_text = std::fs::read_to_string(schema_path)
        .with_context(|| format!("reading {}", schema_path.display()))?;
    let schema_value: JsonValue = match serde_yaml::from_str(&schema_text) {
        Ok(v) => v,
        Err(e) => {
            report.push(
                Finding::new(
                    schema_rule,
                    Severity::Error,
                    format!("{label}.schema.yaml is not valid YAML: {e}"),
                )
                .with_file(schema_path.display().to_string()),
            );
            return Ok(());
        }
    };

    let validator = match jsonschema::options()
        .with_retriever(retriever)
        .build(&schema_value)
    {
        Ok(v) => v,
        Err(e) => {
            report.push(
                Finding::new(
                    schema_rule,
                    Severity::Error,
                    format!("failed to compile {label}.schema.yaml: {e}"),
                )
                .with_file(schema_path.display().to_string()),
            );
            return Ok(());
        }
    };

    let toml_text = std::fs::read_to_string(toml_path)
        .with_context(|| format!("reading {}", toml_path.display()))?;
    let toml_value: JsonValue = match toml::from_str(&toml_text) {
        Ok(v) => v,
        Err(e) => {
            report.push(
                Finding::new(
                    schema_rule,
                    Severity::Error,
                    format!("{} is not valid TOML: {e}", file_label(toml_path)),
                )
                .with_file(toml_path.display().to_string()),
            );
            return Ok(());
        }
    };

    for err in validator.iter_errors(&toml_value) {
        report.push(
            Finding::new(
                schema_rule,
                Severity::Error,
                format!(
                    "{}: {}",
                    file_label(toml_path),
                    crate::diagnostics::format_schema_error(&err)
                ),
            )
            .with_file(toml_path.display().to_string())
            .with_pointer(err.instance_path().to_string()),
        );
    }

    Ok(())
}

/// The file name a schema finding leads with (`requirements.toml`,
/// `DEC-001.toml`).
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Map-backed `Retrieve` for resolving `$ref` between sibling schema
/// files in `specs/specdev/schemas/`. Mirrors `aggregator::MapRetriever`
/// but scoped to the standalone schemas (architecture, requirements,
/// enums, metadata). Inlined block/bridge schemas are out of scope here
/// — those resolve via the aggregator's own retriever.
struct AuxRetriever {
    docs: Arc<HashMap<String, JsonValue>>,
}

impl Retrieve for AuxRetriever {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> std::result::Result<JsonValue, Box<dyn std::error::Error + Send + Sync>> {
        let s = uri.as_str();
        if let Some(v) = self.docs.get(s) {
            return Ok(v.clone());
        }
        if let Some(tail) = s.rsplit('/').next() {
            if let Some(v) = self.docs.get(tail) {
                return Ok(v.clone());
            }
        }
        Err(format!("schema not found in retriever: {s}").into())
    }
}

fn build_aux_retriever_docs(layout: &LayoutContext) -> Result<HashMap<String, JsonValue>> {
    let mut docs: HashMap<String, JsonValue> = HashMap::new();

    // Embedded, project-independent schemas. Registered
    // under both `$id` and filename so `$ref` to either form resolves.
    for (keys, value) in crate::embedded_schemas::embedded_schema_entries() {
        for k in keys {
            docs.insert(k.clone(), value.clone());
        }
    }

    let dir = &layout.schemas_dir;
    if !dir.is_dir() {
        return Ok(docs);
    }
    for entry in std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .flatten()
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if !fname.ends_with(".schema.yaml") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let value: JsonValue = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue, // schema parse errors are surfaced elsewhere
        };
        if let Some(id) = value.get("$id").and_then(JsonValue::as_str) {
            docs.insert(id.to_string(), value.clone());
        }
        docs.insert(fname.to_string(), value);
    }
    Ok(docs)
}

/// Layout directories that may be absent: they hold only units the project
/// inlines and module specs it writes, so a fresh clone has none of them.
const MAY_BE_EMPTY_DIRS: &[&str] = &[
    "specs/specdev/schemas/inlined/blocks/",
    "specs/specdev/schemas/inlined/bridges/",
    "specs/specdev/reductions/blocks/",
    "specs/specdev/reductions/bridges/",
    "specs/specdev/reductions/bridges/local/",
    "specs/modules/",
];

/// Layout pass: enumerate `specs/specdev/`, run the aggregator
/// `$ref` resolver, load reductions, and surface `domain.toml`'s
/// graph-check entry count. Findings only — does not gate later passes.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_layout(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    let inv = layout.enumerate();

    // F-009: suppress missing-layout warnings (LAYOUT-003) for module /
    // requirement entries when the project's profile declares no such
    // surface. The load-bearing signal is `profile.yaml.surface.has_modules`
    // / `surface.has_requirements`; the two suppressions are independently
    // controlled (the schema and authored manifests give us per-surface
    // granularity).
    //
    // The suppression set also covers the `specs/modules/` directory entry
    // when `has_modules: false`. Workspace-root profiles like
    // `rust-workspace` declare no module surface; a missing `specs/modules/`
    // there is by-design, not a layout violation.
    //
    // Backward-compat fallback: when `profile.yaml` is absent OR
    // `manifest.surface` is None, fall back to the legacy
    // `layout.spec.workspace.is_some()` heuristic for both suppressions. The
    // fallback preserves pre-manifest behavior verbatim for existing
    // projects and is removed in v1.1.
    //
    // Malformed-manifest path: load failures surface a single
    // `MANIFEST-001` finding; the suppression then degrades to the
    // legacy heuristic so we don't double-fire a "missing schema"
    // warning on top of "manifest unloadable" noise. Other validate
    // passes continue.
    let (suppress_module, suppress_requirements) = compute_surface_suppressions(layout, report);

    for entry in &inv.directories {
        if entry.exists {
            report.push(Finding::new(
                "LAYOUT-002",
                Severity::Info,
                format!("present: {}", entry.label),
            ));
        } else {
            if suppress_module && entry.label == "specs/modules/" {
                continue;
            }
            // A directory that holds only what the project adds later is
            // created empty, and git does not store an empty directory, so
            // every clone lacks it. Absent means "none yet", not missing.
            if MAY_BE_EMPTY_DIRS.contains(&entry.label.as_str()) {
                continue;
            }
            report.push(Finding::new(
                "LAYOUT-003",
                Severity::Warning,
                format!(
                    "missing: {} (expected at {})",
                    entry.label,
                    entry.path.display()
                ),
            ));
        }
    }

    for entry in &inv.files {
        if entry.exists {
            report.push(Finding::new(
                "LAYOUT-002",
                Severity::Info,
                format!("present: {}", entry.label),
            ));
        } else {
            // Surface-driven suppressions. Each schema is
            // independently gated; the manifest authors per-schema control.
            if suppress_module && entry.label == "specs/specdev/schemas/module.schema.yaml" {
                continue;
            }
            if suppress_requirements
                && entry.label == "specs/specdev/schemas/requirements.schema.yaml"
            {
                continue;
            }
            report.push(Finding::new(
                "LAYOUT-003",
                Severity::Warning,
                format!(
                    "missing: {} (expected at {})",
                    entry.label,
                    entry.path.display()
                ),
            ));
        }
    }
    // Aggregator $ref resolver.
    let _agg = aggregator::resolve(layout, report)?;

    // Reduction loader.
    let inv = reductions::load_all(layout)?;
    if let Some(p) = &inv.project {
        report.push(Finding::new(
            "REDUCTION-001",
            Severity::Info,
            format!(
                "loaded project reduction from {} ({} kind declaration(s))",
                p.source_path.display(),
                p.kind_declarations.len()
            ),
        ));
    }
    for r in inv
        .blocks
        .iter()
        .chain(inv.bridges_canonical.iter())
        .chain(inv.bridges_local.iter())
    {
        report.push(Finding::new(
            "REDUCTION-001",
            Severity::Info,
            format!(
                "loaded {} reduction from {} ({} kind declaration(s))",
                r.namespace.label(),
                r.source_path.display(),
                r.kind_declarations.len()
            ),
        ));
    }

    // domain.toml: parse, gather graph-check entries, do not validate kinds.
    if layout.domain_toml_path.is_file() {
        match DomainConfig::load(&layout.domain_toml_path) {
            Ok(dom) => {
                let count = dom.module.graph_checks.len() + dom.project.graph_checks.len();
                report.push(Finding::new(
                    "DOMAIN-001",
                    Severity::Info,
                    format!("loaded domain.toml ({} graph-check entries)", count),
                ));
            }
            Err(e) => {
                report.push(
                    Finding::new(
                        "DOMAIN-002",
                        Severity::Error,
                        format!("failed to parse domain.toml: {e:#}"),
                    )
                    .with_file(layout.domain_toml_path.display().to_string()),
                );
            }
        }
    }

    Ok(())
}

/// Module pass.
///
/// 1. Build the [`EffectiveKindSet`] via [`reductions::resolve`] and merge
///    its findings into `report` (METASCHEMA-001/002/003/004/005).
/// 2. Compile the aggregator schema via [`aggregator::compile`]; emit
///    AGGREGATOR-005 if compilation fails. Cycles surface as
///    AGGREGATOR-003 errors during the resolve step.
/// 3. Validate every module TOML under `specs/modules/` against the
///    compiled aggregated schema; emit `SCHEMA` findings.
/// 4. Resolve `domain.toml` graph-check kind references against the
///    [`EffectiveKindSet`]; emit DOMAIN-003 for unresolved references.
/// 5. Run the reduction executor against the kind set and execute
///    `domain.toml` graph_checks against the resulting graph, with
///    transitive-extends matching when a check's `scope` names a
///    canonical (or qualified) kind.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_modules(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    let inv = layout.enumerate();

    // Stage 1: build effective kind set.
    let raw_inv = reductions::load_all(layout)?;
    let (kindset, kindset_findings) = reductions::resolve(&raw_inv);
    report.extend(kindset_findings);

    // Stage 2: compile aggregator schema.
    let validator = aggregator::compile(layout, report)?;

    // Stage 3: validate each module TOML.
    for module_path in &inv.modules {
        let rel = module_path
            .strip_prefix(&layout.project_root)
            .unwrap_or(module_path);
        let text = match std::fs::read_to_string(module_path) {
            Ok(t) => t,
            Err(e) => {
                report.push(
                    Finding::new(
                        "LAYOUT-005",
                        Severity::Error,
                        format!("failed to read module {}: {e}", module_path.display()),
                    )
                    .with_file(module_path.display().to_string()),
                );
                continue;
            }
        };
        let parsed: toml::Value = match toml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                report.push(
                    Finding::new(
                        "LAYOUT-005",
                        Severity::Error,
                        format!("module {} TOML parse error: {e}", module_path.display()),
                    )
                    .with_file(module_path.display().to_string()),
                );
                continue;
            }
        };
        let json = match serde_json::to_value(&parsed) {
            Ok(v) => v,
            Err(e) => {
                report.push(
                    Finding::new(
                        "LAYOUT-005",
                        Severity::Error,
                        format!("module {} TOML→JSON failed: {e}", module_path.display()),
                    )
                    .with_file(module_path.display().to_string()),
                );
                continue;
            }
        };
        let module_name = parsed
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or("<unnamed>");
        info!(
            target: "specdev_analyzer::validate",
            "[MODULE-001] loaded {} — kind=Module name={}",
            rel.display(),
            module_name,
        );
        if let Some(v) = &validator {
            for err in v.iter_errors(&json) {
                report.push(
                    Finding::new(
                        "SCHEMA",
                        Severity::Error,
                        crate::diagnostics::format_schema_error(&err),
                    )
                    .with_file(module_path.display().to_string())
                    .with_pointer(err.instance_path().to_string()),
                );
            }
        } else {
            report.push(Finding::new(
                "LAYOUT-005",
                Severity::Info,
                format!(
                    "module {} (no aggregator schema available — module-level structural \
                     validation skipped)",
                    module_path.display()
                ),
            ));
        }

        validate_context_files(layout, &parsed, module_path, report);
    }

    // Stage 4: domain.toml graph-check resolution + execution.
    if layout.domain_toml_path.is_file() {
        match DomainConfig::load(&layout.domain_toml_path) {
            Ok(dom) => {
                resolve_graph_check_kinds(
                    &dom.module.graph_checks,
                    &kindset,
                    &layout.domain_toml_path,
                    "module",
                    report,
                );
                resolve_graph_check_kinds(
                    &dom.project.graph_checks,
                    &kindset,
                    &layout.domain_toml_path,
                    "project",
                    report,
                );

                // Stage 5: run executor + project graph_checks.
                let mut exec_report = Report::default();
                let graph = match reduction::execute_kindset(&kindset, layout, &mut exec_report) {
                    Ok(graph) => graph,
                    Err(e) => {
                        // A spec file that does not parse (a module, or
                        // `decisions.toml` and its siblings) stops the
                        // reduction. Report it as a counted error and keep
                        // every other finding: aborting here dropped the
                        // whole report, `LAYOUT-005` included, and printed
                        // no count line (#196). The graph checks cannot run
                        // without the graph, and the finding says so.
                        report.extend(exec_report.findings().to_vec());
                        report.push(Finding::new(
                            "REDUCE-001",
                            Severity::Error,
                            format!(
                                "the entity graph could not be built, so no graph check ran: \
                                 {e:#}"
                            ),
                        ));
                        return Ok(());
                    }
                };
                report.extend(exec_report.findings().to_vec());

                execute_graph_checks(
                    &dom.project.graph_checks,
                    "domain.toml [project]",
                    &BTreeMap::new(),
                    &kindset,
                    &graph,
                    &dom,
                    layout,
                    report,
                );
                // Each inlined rule pack's graph checks, with the pack's
                // default severities under the project's.
                let (packs, load_findings) = crate::packs::load_inlined(&layout.specdev_root);
                report.extend(load_findings);
                for pack in &packs {
                    report.push(Finding::new(
                        "PACK-000",
                        Severity::Info,
                        format!(
                            "loaded rule pack {} ({} graph check(s), {} code check(s))",
                            pack.path.display(),
                            pack.file.graph_checks.len(),
                            pack.file.code_checks.len()
                        ),
                    ));
                    let label = format!("rule pack {}", pack.path.display());
                    resolve_check_kinds(
                        &pack.file.graph_checks,
                        &kindset,
                        &pack.path,
                        &label,
                        true,
                        report,
                    );
                    execute_graph_checks(
                        &pack.file.graph_checks,
                        &label,
                        &pack.file.rules,
                        &kindset,
                        &graph,
                        &dom,
                        layout,
                        report,
                    );
                }
                // Module-scoped checks are reserved for per-module
                // execution in a future phase; emit an info instead of
                // running them at the project level.
                for check in &dom.module.graph_checks {
                    let rule = check.rule.as_deref().unwrap_or("?");
                    report.push(Finding::new(
                        "DOMAIN-001",
                        Severity::Info,
                        format!(
                            "module-scope graph_check `{rule}` registered (module-scoped \
                             execution lands when per-module scoping wires through the \
                             open-vocabulary executor)",
                        ),
                    ));
                }
            }
            Err(_) => {
                // The DOMAIN-002 parse error is already emitted by
                // validate_layout; don't double-report.
            }
        }
    }

    Ok(())
}

/// Validate `module.context_files`: each declared
/// path is spec-root-relative, must exist, must be readable, and must be
/// valid UTF-8. Schema validation upstream guarantees the field shape
/// (string array, lex-unique). This function adds the per-file
/// filesystem checks: `CONTEXT-001` for missing files, `CONTEXT-002`
/// for I/O or UTF-8 failures.
fn validate_context_files(
    layout: &LayoutContext,
    parsed: &toml::Value,
    module_path: &Path,
    report: &mut Report,
) {
    let context_files: Vec<String> = parsed
        .get("context_files")
        .and_then(toml::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    for rel in &context_files {
        let abs = layout.project_root.join(rel);
        if !abs.is_file() {
            report.push(
                Finding::new(
                    "CONTEXT-001",
                    Severity::Error,
                    format!(
                        "module.context_files entry `{rel}` does not exist at \
                         {} (paths are spec-root-relative)",
                        abs.display()
                    ),
                )
                .with_file(module_path.display().to_string()),
            );
            continue;
        }
        if let Err(e) = std::fs::read_to_string(&abs) {
            report.push(
                Finding::new(
                    "CONTEXT-002",
                    Severity::Error,
                    format!(
                        "module.context_files entry `{rel}` could not be read \
                         as UTF-8: {e}"
                    ),
                )
                .with_file(module_path.display().to_string()),
            );
        }
    }
}

/// Run domain.toml graph_checks against the executor's project graph.
///
/// The executor's [`Graph`] is serialized to a JSON value and walked via
/// the existing selector primitives in [`crate::graph`]. Two
/// adaptations:
///
/// 1. **Transitive-extends pre-filtering.** When a check's `scope` field
///    names a canonical or qualified kind, the relations / entities in the
///    JSON view are filtered to those whose `extends` chain reaches the
///    target. A check on canonical `depends_on` matches any relation
///    whose chain reaches `depends_on` (e.g. `embassy.spawns`); a check
///    on `embassy.spawns` matches only that exact qualified path.
/// 2. **Primitive dispatch.** `check.kind` names the primitive
///    (`unique` / `reference_exists` / `reachability` / `acyclic` /
///    `layer_dependency`); the check's selector fields drive the call.
///    Unknown primitives surface as `DOMAIN-003` warnings so authoring
///    typos surface visibly.
#[allow(clippy::too_many_arguments)]
fn execute_graph_checks(
    checks: &[GraphCheck],
    section: &str,
    pack_rules: &BTreeMap<String, Severity>,
    kindset: &EffectiveKindSet,
    graph: &Graph,
    domain: &DomainConfig,
    layout: &LayoutContext,
    report: &mut Report,
) {
    let graph_json = match serde_json::to_value(graph) {
        Ok(v) => v,
        Err(_) => return,
    };

    for check in checks {
        let rule = check.rule.as_deref().unwrap_or("?");
        let severity = crate::packs::severity(&domain.validation.rules, pack_rules, rule);
        let primitive = check.kind.as_str();

        let view = filter_graph_for_check(&graph_json, check, kindset);

        let findings = match primitive {
            "unique" => match (check.fields.as_deref(), check.field.as_deref()) {
                (Some(fields), _) => graph_checks::unique_fields(&view, fields, rule, severity),
                (None, Some(f)) => graph_checks::unique(&view, f, rule, severity),
                (None, None) => vec![Finding::new(
                    "DOMAIN-003",
                    Severity::Error,
                    format!("{section} graph_check `{rule}` (unique) needs `field` or `fields`; it checked nothing"),
                )],
            },
            "related_attrs_allowed" => {
                match (
                    check.relation_scope.as_deref(),
                    check.left.as_deref(),
                    check.right.as_deref(),
                    check.allowed.as_deref(),
                ) {
                    (Some(rs), Some(l), Some(r), Some(allowed)) => {
                        let view = relations_of_kind(&graph_json, rs, kindset);
                        graph_checks::related_attrs_allowed(&view, l, r, allowed, rule, severity)
                    }
                    _ => vec![Finding::new(
                        "DOMAIN-003",
                        Severity::Error,
                        format!(
                            "{section} graph_check `{rule}` (related_attrs_allowed) needs `relation_scope`, \
                             `left`, `right` and `allowed`; it checked nothing"
                        ),
                    )],
                }
            }
            "reference_exists" => {
                match (check.source_field.as_deref(), check.target_field.as_deref()) {
                    (Some(s), Some(t)) => {
                        graph_checks::reference_exists(&view, s, t, rule, severity)
                    }
                    _ => Vec::new(),
                }
            }
            "reachability" => match (
                check.nodes_field.as_deref(),
                check.edges_from.as_deref(),
                check.edges_to.as_deref(),
                check.entry_field.as_deref(),
            ) {
                (Some(n), Some(ef), Some(et), Some(e)) => {
                    graph_checks::reachability(&view, n, ef, et, e, rule, severity)
                }
                _ => Vec::new(),
            },
            "acyclic" => match (check.edges_from.as_deref(), check.edges_to.as_deref()) {
                (Some(ef), Some(et)) => {
                    // Optional joint filter: when both entity_scope and
                    // relation_scope are set, retain only entities of
                    // the entity kind AND only relations whose endpoints
                    // are in that entity set AND whose kind extends the
                    // relation kind. Lets module-deps acyclicity exclude
                    // broader canonical-`depends_on` chains (e.g. triad
                    // relations from Requirement → Decision) by scoping
                    // to (Module, depends_on) — only Module → Module
                    // edges remain in view.
                    let scoped_view = if let (Some(es), Some(rs)) = (
                        check.entity_scope.as_deref(),
                        check.relation_scope.as_deref(),
                    ) {
                        let mut v = filter_for_layer_dependency(&graph_json, es, rs, kindset);
                        // Drop relations whose endpoints aren't in the
                        // filtered entity set — chain-walked relation
                        // kinds otherwise carry edges to entities of a
                        // different kind than entity_scope.
                        let entity_names: BTreeSet<String> = v
                            .get("entities")
                            .and_then(|e| e.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|e| {
                                        e.get("name").and_then(|n| n.as_str()).map(str::to_string)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        if let Some(rels) = v.get_mut("relations").and_then(|r| r.as_array_mut()) {
                            rels.retain(|r| {
                                let from_in = r
                                    .get("from")
                                    .and_then(|f| f.as_str())
                                    .map(|s| entity_names.contains(s))
                                    .unwrap_or(false);
                                let to_in = r
                                    .get("to")
                                    .and_then(|t| t.as_str())
                                    .map(|s| entity_names.contains(s))
                                    .unwrap_or(false);
                                from_in && to_in
                            });
                        }
                        v
                    } else {
                        view.clone()
                    };
                    graph_checks::acyclic(&scoped_view, ef, et, rule, severity)
                }
                _ => Vec::new(),
            },
            "layer_dependency" => layer_dependency_dispatch(
                graph_json_full(&graph_json, &view),
                check,
                kindset,
                domain,
                rule,
                severity,
            ),
            // F-006: jointly filter by entity_scope + relation_scope and
            // run the orphan-requirements check. Reuses the same joint-
            // filter infrastructure as `layer_dependency`.
            //
            // F-003 (zfm dogfood): when a project has zero modules
            // (typical of a docs-first port mid-conversion), every
            // `nature = "module"` requirement is guaranteed orphan and
            // emitting one warning per requirement produces a wall of
            // noise. Collapse into a single summary line in that case;
            // populated trees keep per-requirement diagnostics.
            "requirements_have_satisfiers" => {
                match (
                    check.entity_scope.as_deref(),
                    check.relation_scope.as_deref(),
                ) {
                    (Some(es), Some(rs)) => {
                        let view = filter_for_layer_dependency(&graph_json, es, rs, kindset);
                        let mut findings =
                            graph_checks::requirements_have_satisfiers(&view, rule, severity);
                        if findings.len() > 1 && count_modules(&graph_json) == 0 {
                            let count = findings.len();
                            findings = vec![Finding::new(
                                rule,
                                severity,
                                format!(
                                    "{count} requirements (nature = \"module\") have no module \
                                     satisfier — project has zero modules. Add modules under \
                                     `specs/modules/` and reference them via `satisfies = [...]`, \
                                     or set `nature = \"architecture\" | \"build\" | \"test\"` to \
                                     exempt requirements that don't map to a module."
                                ),
                            )];
                        }
                        findings
                    }
                    _ => Vec::new(),
                }
            }
            // `entity_has_inbound_relation`: the generic orphan
            // check. Joint-filter entities by `entity_scope` and relations
            // by `relation_scope`, then flag any in-scope entity that is
            // never a relation target. Reuses the same joint-filter
            // infrastructure as `requirements_have_satisfiers` but with a
            // vocabulary-neutral diagnostic (audio-plugin's
            // `parameters_have_bindings` is the first consumer).
            "entity_has_inbound_relation" => {
                match (
                    check.entity_scope.as_deref(),
                    check.relation_scope.as_deref(),
                ) {
                    (Some(es), Some(rs)) => {
                        // Fail closed when `entity_scope` names a kind that
                        // isn't in the effective set — e.g. `nih-plug.Parameter`
                        // before the nih-plug block is inlined. `entity_scope`
                        // here is contractually an entity kind, so an
                        // unresolvable one means there are no in-scope
                        // entities to check. Without this guard,
                        // `filter_for_layer_dependency` fails OPEN for an
                        // unknown kind (it skips entity filtering and keeps the
                        // full graph), and the orphan primitive then flags
                        // every unrelated entity — Sources, requirements,
                        // decisions, open questions — as missing an inbound
                        // relation. (The filter's fail-open is intentional for
                        // checks whose scope is deliberately not a kind, so the
                        // guard lives here, not in the shared filter.)
                        let entity_scope_known = kinds::parse(es)
                            .ok()
                            .map(|p| {
                                let key = p.to_string();
                                kindset.canonical_roots.contains(&key)
                                    || kindset.entity(&key).is_some()
                            })
                            .unwrap_or(false);
                        if !entity_scope_known {
                            Vec::new()
                        } else {
                            let view = filter_for_layer_dependency(&graph_json, es, rs, kindset);
                            graph_checks::entity_has_inbound_relation(
                                &view,
                                rs,
                                check.entity_attr.as_deref(),
                                check.entity_attr_value.as_deref(),
                                rule,
                                severity,
                            )
                        }
                    }
                    _ => Vec::new(),
                }
            }
            // source-aware check. Each
            // `requirement.verified_by[]` entry resolves against tree-sitter-
            // extracted top-level items + #[test] items per module. Unlike
            // the other primitives, this one reads filesystem source via
            // the items extraction surface. The warning promotes to error
            // when items[] is the authoritative substrate.
            // `verified_by` resolves against the test-discovery pass over the
            // source, so `check` runs it, not the spec-only pass.
            "verified_by_resolves" => Vec::new(),
            // `decision_supersedes_chain_consistency`: filter
            // relations by `relation_scope`, then for each entity matching
            // the source-side filter, count inbound relations and compare
            // to `expected_count`.
            "entity_attr_inbound_count" => match (
                check.entity_attr.as_deref(),
                check.entity_attr_value.as_deref(),
                check.relation_scope.as_deref(),
                check.expected_count,
            ) {
                (Some(ea), Some(eav), Some(rs), Some(exp)) => {
                    // Joint-filter the graph view: entity_scope filters
                    // entities[], relation_scope filters relations[].
                    let entity_scope = check.entity_scope.as_deref().unwrap_or("");
                    let view = filter_for_layer_dependency(&graph_json, entity_scope, rs, kindset);
                    graph_checks::entity_attr_inbound_count(&view, ea, eav, exp, rule, severity)
                }
                _ => Vec::new(),
            },
            "entity_count" => match (
                check.entity_attr.as_deref(),
                check.entity_attr_value.as_deref(),
                check.expected_count,
            ) {
                (Some(ea), Some(eav), Some(exp)) => {
                    let entity_scope = check.entity_scope.as_deref().unwrap_or("");
                    let view = filter_for_layer_dependency(&graph_json, entity_scope, "", kindset);
                    graph_checks::entity_count(&view, entity_scope, ea, eav, exp, rule, severity)
                }
                _ => Vec::new(),
            },
            // `open_question_status_consistency`: cross-entity
            // attr-target-attr consistency check.
            "entity_attr_target_attr_equals" => match (
                check.entity_scope.as_deref(),
                check.entity_attr.as_deref(),
                check.entity_attr_value.as_deref(),
                check.target_attr_field.as_deref(),
                check.target_entity_kind.as_deref(),
                check.target_attr.as_deref(),
                check.target_attr_value.as_deref(),
            ) {
                (Some(ek), Some(ea), Some(eav), Some(taf), Some(tek), Some(ta), Some(tav)) => {
                    graph_checks::entity_attr_target_attr_equals(
                        &graph_json,
                        ek,
                        ea,
                        eav,
                        taf,
                        tek,
                        ta,
                        tav,
                        rule,
                        severity,
                    )
                }
                _ => Vec::new(),
            },
            // `codegen_unblocked` validate-time warning: walks
            // Module.satisfies → Requirement → OpenQuestion.affects^-1
            // and fires when any reachable OQ has status = open. The
            // gen-side hard refusal lives in `gen.rs`; this primitive
            // mirrors the reachability informational at validate time.
            // The open-question gate keys on a module's source existing and
            // reads workspace-tier questions, so `check` runs it, not the
            // spec-only pass.
            "module_reachable_through_open_question" => Vec::new(),
            "ids_in_document" => ids_in_document(&graph_json, check, kindset, layout, rule, severity),
            other => vec![Finding::new(
                "DOMAIN-003",
                Severity::Warning,
                format!(
                    "{section} graph_check (rule `{rule}`): unknown primitive `{other}` — \
                     supported: {}",
                    GRAPH_CHECK_PRIMITIVES.join(", ")
                ),
            )],
        };

        report.extend(findings);
    }
}

/// The graph-check primitives `execute_graph_checks` dispatches on.
/// `plugin-lint` refuses a rule pack that names any other.
pub const GRAPH_CHECK_PRIMITIVES: &[&str] = &[
    "unique",
    "reference_exists",
    "reachability",
    "acyclic",
    "layer_dependency",
    "requirements_have_satisfiers",
    "entity_has_inbound_relation",
    "verified_by_resolves",
    "entity_attr_inbound_count",
    "entity_count",
    "entity_attr_target_attr_equals",
    "module_reachable_through_open_question",
    "ids_in_document",
    "related_attrs_allowed",
];

/// The graph with its relations kept to those whose kind extends `scope`
/// (all entities kept, so endpoints resolve). An unknown `scope` keeps no
/// relation: the check fails closed.
fn relations_of_kind(graph_json: &JsonValue, scope: &str, kindset: &EffectiveKindSet) -> JsonValue {
    let key = kinds::parse(scope)
        .map(|p| p.to_string())
        .unwrap_or_else(|_| scope.to_string());
    let mut view = graph_json.clone();
    if let Some(rels) = view.get_mut("relations").and_then(|v| v.as_array_mut()) {
        rels.retain(|r| {
            r.get("kind")
                .and_then(|k| k.as_str())
                .is_some_and(|k| relation_extends_to(kindset, k, &key))
        });
    }
    view
}

/// IDS_IN_DOCUMENT: every entity in `entity_scope` (optionally only those
/// whose `entity_attr` equals `entity_attr_value`) has its id appear in
/// `document`, a file relative to the project root, as a whole token.
fn ids_in_document(
    graph_json: &JsonValue,
    check: &GraphCheck,
    kindset: &EffectiveKindSet,
    layout: &LayoutContext,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    let (Some(scope), Some(document)) = (check.entity_scope.as_deref(), check.document.as_deref())
    else {
        return vec![Finding::new(
            "DOMAIN-003",
            Severity::Warning,
            format!("graph_check `{rule}` (ids_in_document) needs `entity_scope` and `document`"),
        )];
    };
    let Ok(parsed) = kinds::parse(scope) else {
        return Vec::new();
    };
    let key = parsed.to_string();
    let path = layout.project_root.join(document);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return vec![Finding::new(
            rule,
            severity,
            format!("`{rule}`: the document {} cannot be read", path.display()),
        )];
    };
    let mut out = Vec::new();
    for ent in graph_json
        .get("entities")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let (Some(kind), Some(name)) = (
            ent.get("kind").and_then(|v| v.as_str()),
            ent.get("name").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        if !entity_extends_to(kindset, kind, &key) {
            continue;
        }
        if let (Some(attr), Some(want)) = (
            check.entity_attr.as_deref(),
            check.entity_attr_value.as_deref(),
        ) {
            let have = ent
                .get("attrs")
                .and_then(|a| a.get(attr))
                .and_then(|v| v.as_str());
            if have != Some(want) {
                continue;
            }
        }
        if !contains_token(&text, name) {
            out.push(
                Finding::new(
                    rule,
                    severity,
                    format!("`{name}` does not appear in {document}"),
                )
                .with_entity(name),
            );
        }
    }
    out
}

/// `needle` occurs in `hay` with no identifier character (`[A-Za-z0-9_-]`)
/// directly before or after it, so `DEC-1` does not match inside `DEC-10`.
fn contains_token(hay: &str, needle: &str) -> bool {
    let is_id = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    hay.match_indices(needle).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + needle.len()..].chars().next();
        !before.is_some_and(is_id) && !after.is_some_and(is_id)
    })
}

/// Returns whichever JSON value the layer_dependency dispatch should
/// operate on. The pre-filter from `filter_graph_for_check` collapses the
/// `scope` field to one kind (entity-only OR relation-only), but
/// layer_dependency needs simultaneous filtering of entities (by
/// `entity_scope`) and relations (by `relation_scope`). We always start
/// from the unfiltered graph and let `filter_for_layer_dependency` do
/// its own joint filtering.
fn graph_json_full<'a>(full: &'a JsonValue, _scope_filtered: &'a JsonValue) -> &'a JsonValue {
    full
}

/// Dispatch for the layer_dependency primitive.
///
/// Two execution modes coexist:
///
/// 1. **Graph-shape mode (preferred):** `entity_scope` +
///    `relation_scope` + `layer_field` + `layer_ordering`. The graph is
///    jointly filtered (entities of the entity kind, relations of the
///    relation kind) and [`graph_checks::layer_dependency_graph`] is
///    invoked.
/// 2. **Legacy mode:** `module_layer_field` + `module_deps_field` +
///    `[layers.allowed_deps]` table. Source-data shape; retained for
///    legacy callers and the existing unit test.
fn layer_dependency_dispatch(
    full_graph: &JsonValue,
    check: &GraphCheck,
    kindset: &EffectiveKindSet,
    domain: &DomainConfig,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    if let (Some(entity_scope), Some(relation_scope), Some(layer_field), Some(ordering)) = (
        check.entity_scope.as_deref(),
        check.relation_scope.as_deref(),
        check.layer_field.as_deref(),
        check.layer_ordering.as_ref(),
    ) {
        let view = filter_for_layer_dependency(full_graph, entity_scope, relation_scope, kindset);
        return graph_checks::layer_dependency_graph(&view, layer_field, ordering, rule, severity);
    }

    // Legacy fallback.
    if let (Some(lf), Some(df)) = (
        check.module_layer_field.as_deref(),
        check.module_deps_field.as_deref(),
    ) {
        return graph_checks::layer_dependency(
            full_graph,
            lf,
            df,
            &domain.layers.allowed_deps,
            rule,
            severity,
        );
    }

    Vec::new()
}

/// Filter the graph view jointly by `entity_scope` (applies to
/// `entities[]`) and `relation_scope` (applies to `relations[]`). Used by
/// the graph-shape `layer_dependency` dispatch which needs both filters
/// at once.
pub(crate) fn filter_for_layer_dependency(
    graph_json: &JsonValue,
    entity_scope: &str,
    relation_scope: &str,
    kindset: &EffectiveKindSet,
) -> JsonValue {
    let mut filtered = graph_json.clone();

    if let Ok(parsed) = kinds::parse(entity_scope) {
        let key = parsed.to_string();
        let is_entity = kindset.canonical_roots.contains(&key) || kindset.entity(&key).is_some();
        if is_entity {
            if let Some(entities) = filtered.get_mut("entities").and_then(|v| v.as_array_mut()) {
                entities.retain(|e| {
                    let Some(ekind) = e.get("kind").and_then(|v| v.as_str()) else {
                        return false;
                    };
                    entity_extends_to(kindset, ekind, &key)
                });
            }
        }
    }

    if let Ok(parsed) = kinds::parse(relation_scope) {
        let key = parsed.to_string();
        let is_relation =
            kindset.canonical_relations.contains(&key) || kindset.relation(&key).is_some();
        if is_relation {
            if let Some(relations) = filtered.get_mut("relations").and_then(|v| v.as_array_mut()) {
                relations.retain(|r| {
                    let Some(rkind) = r.get("kind").and_then(|v| v.as_str()) else {
                        return false;
                    };
                    relation_extends_to(kindset, rkind, &key)
                });
            }
        }
    }

    filtered
}

/// Pre-filter the executor's graph JSON view based on the check's `scope`
/// field. When `scope` parses as a kind path that resolves in the
/// effective set, relations or entities are filtered to those whose
/// extends chain reaches the target. Checks without a kind-shaped scope
/// see the full graph.
fn filter_graph_for_check(
    graph_json: &JsonValue,
    check: &GraphCheck,
    kindset: &EffectiveKindSet,
) -> JsonValue {
    let Some(scope) = check.scope.as_deref() else {
        return graph_json.clone();
    };
    let Ok(parsed) = kinds::parse(scope) else {
        return graph_json.clone();
    };
    let key = parsed.to_string();

    let is_relation =
        kindset.canonical_relations.contains(&key) || kindset.relation(&key).is_some();
    let is_entity = kindset.canonical_roots.contains(&key) || kindset.entity(&key).is_some();

    if !(is_relation || is_entity) {
        // Already flagged as DOMAIN-003 in resolve_graph_check_kinds.
        return graph_json.clone();
    }

    let mut filtered = graph_json.clone();
    if is_relation {
        if let Some(relations) = filtered.get_mut("relations").and_then(|v| v.as_array_mut()) {
            relations.retain(|r| {
                let Some(rkind) = r.get("kind").and_then(|v| v.as_str()) else {
                    return false;
                };
                relation_extends_to(kindset, rkind, &key)
            });
        }
    } else if let Some(entities) = filtered.get_mut("entities").and_then(|v| v.as_array_mut()) {
        entities.retain(|e| {
            let Some(ekind) = e.get("kind").and_then(|v| v.as_str()) else {
                return false;
            };
            entity_extends_to(kindset, ekind, &key)
        });
    }
    filtered
}

fn relation_extends_to(kindset: &EffectiveKindSet, rkind: &str, target: &str) -> bool {
    if rkind == target {
        return true;
    }
    let mut cursor = rkind.to_string();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while seen.insert(cursor.clone()) {
        let Some(decl) = kindset.relation(&cursor) else {
            break;
        };
        match &decl.extends {
            ResolvedExtends::Canonical(s) => return s == target,
            ResolvedExtends::Qualified(p) => {
                let s = p.to_string();
                if s == target {
                    return true;
                }
                cursor = s;
            }
        }
    }
    false
}

fn entity_extends_to(kindset: &EffectiveKindSet, ekind: &str, target: &str) -> bool {
    if ekind == target {
        return true;
    }
    let mut cursor = ekind.to_string();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while seen.insert(cursor.clone()) {
        let Some(decl) = kindset.entity(&cursor) else {
            break;
        };
        match &decl.extends {
            ResolvedExtends::Canonical(s) => return s == target,
            ResolvedExtends::Qualified(p) => {
                let s = p.to_string();
                if s == target {
                    return true;
                }
                cursor = s;
            }
        }
    }
    false
}

/// Resolve every kind-string field of every graph-check entry against
/// the [`EffectiveKindSet`]. The check's primitive `kind` field is *not*
/// a kind reference (it names the check primitive: `acyclic`,
/// `reachability`, etc.); the kind references live in the selector
/// fields. Load-time resolution takes whichever fields hold
/// kind paths, parses them, and verifies they resolve in the effective
/// set. Unresolved → `DOMAIN-003`.
///
/// Selector fields treated as kind references (when non-empty):
/// `scope`, `field`, `source_field`, `target_field`, `nodes_field`,
/// `edges_from`, `edges_to`. The selectors as written in `domain.toml`
/// historically named entity/relation kinds; in the open-vocabulary
/// regime they may be qualified paths. Unrecognized fields are ignored
/// (forward-compatible with new graph-check primitives).
fn resolve_graph_check_kinds(
    checks: &[GraphCheck],
    kindset: &EffectiveKindSet,
    domain_toml_path: &Path,
    section: &str,
    report: &mut Report,
) {
    resolve_check_kinds(
        checks,
        kindset,
        domain_toml_path,
        &format!("domain.toml [{section}]"),
        false,
        report,
    );
}

/// As [`resolve_graph_check_kinds`], labelled `label`. With `scopes`, the
/// `entity_scope` and `relation_scope` fields are resolved too: a rule
/// pack's checks name its own unit's kinds, which are inlined with it, so an
/// unresolved one there is a typo. (`domain.toml` checks may name a unit
/// kind before the unit is inlined; those primitives fail closed instead.)
fn resolve_check_kinds(
    checks: &[GraphCheck],
    kindset: &EffectiveKindSet,
    file: &Path,
    label: &str,
    scopes: bool,
    report: &mut Report,
) {
    for (idx, check) in checks.iter().enumerate() {
        let mut candidates: Vec<&Option<String>> = vec![
            &check.scope,
            &check.field,
            &check.source_field,
            &check.target_field,
            &check.nodes_field,
            &check.edges_from,
            &check.edges_to,
        ];
        if scopes {
            candidates.extend([&check.entity_scope, &check.relation_scope]);
        }
        for candidate in candidates.iter().copied().flatten() {
            // A field selector like `[*].kind` or `name` is not a kind
            // reference; only strings that parse as a kind path are
            // resolved. Skip the rest silently.
            let Ok(parsed) = kinds::parse(candidate) else {
                continue;
            };
            let key = parsed.to_string();
            let found_entity =
                kindset.entity(&key).is_some() || kindset.canonical_roots.contains(&key);
            let found_relation =
                kindset.relation(&key).is_some() || kindset.canonical_relations.contains(&key);
            if !(found_entity || found_relation) {
                report.push(
                    Finding::new(
                        "DOMAIN-003",
                        Severity::Error,
                        format!(
                            "{label} graph_check #{idx} (rule `{}`): \
                             kind reference `{candidate}` is not in the effective kind set",
                            check.rule.as_deref().unwrap_or("?"),
                        ),
                    )
                    .with_file(file.display().to_string()),
                );
            }
        }
    }
}

/// Positive-confirmation summary of what `validate` actually loaded.
///
/// Surfaces the per-category counts the user can sanity-check ("did my
/// new module file actually get picked up?") without grepping through
/// findings or running `reduce`. Computed independently of the Report
/// so identical signals are emitted whether or not findings were
/// produced.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Summary {
    pub modules: usize,
    pub requirements: usize,
    pub sources: usize,
    pub graph_checks: usize,
    pub decisions: usize,
    pub open_questions: usize,
    /// Subset of `open_questions` with `status = "open"` — the ones
    /// that gate codegen for the modules they reach.
    pub open_questions_open: usize,
}

impl Summary {
    /// One-line rendering for the CLI tail. The triad counts append
    /// only when at least one triad file has entries, so pre-triad
    /// projects keep the original four-field line.
    pub fn render(&self) -> String {
        let mut line = format!(
            "loaded {} modules / {} requirements / {} sources / {} graph checks",
            self.modules, self.requirements, self.sources, self.graph_checks,
        );
        if self.decisions > 0 || self.open_questions > 0 {
            line.push_str(&format!(
                " / {} decisions / {} open questions ({} open)",
                self.decisions, self.open_questions, self.open_questions_open,
            ));
        }
        line
    }
}

/// Compute the validate summary for a layout. Reads requirements.toml
/// and domain.toml directly (no reduction execution). Treats missing or
/// unparseable files as zero counts — the file's structural validity
/// is the responsibility of the main validate pipeline; summarize is a
/// best-effort positive-confirmation surface.
pub fn summarize(layout: &LayoutContext) -> Summary {
    let inv = layout.enumerate();
    let modules = inv.modules.len();

    let (requirements, sources) = count_requirements_and_sources(layout);
    let graph_checks = count_graph_checks(layout);
    let decisions = EntryStore::for_layout(layout)
        .load_lenient(EntryKind::Decision)
        .len();
    let (open_questions, open_questions_open) = count_open_questions(layout);

    Summary {
        modules,
        requirements,
        sources,
        graph_checks,
        decisions,
        open_questions,
        open_questions_open,
    }
}

/// Best-effort (total, open) counts over the open questions. Unreadable
/// files count as zero.
fn count_open_questions(layout: &LayoutContext) -> (usize, usize) {
    let all = EntryStore::for_layout(layout).load_lenient(EntryKind::OpenQuestion);
    let open = all
        .iter()
        .filter(|q| q.value.get("status").and_then(toml::Value::as_str) == Some("open"))
        .count();
    (all.len(), open)
}

/// Count entities of kind `Module` in the unfiltered graph JSON.
/// F-003: used to decide whether to collapse `requirements_have_satisfiers`
/// findings into a single summary line on a zero-module project.
fn count_modules(graph_json: &JsonValue) -> usize {
    graph_json
        .get("entities")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|e| e.get("kind").and_then(|k| k.as_str()) == Some("Module"))
                .count()
        })
        .unwrap_or(0)
}

fn count_requirements_and_sources(layout: &LayoutContext) -> (usize, usize) {
    let store = EntryStore::for_layout(layout);
    (
        store.load_lenient(EntryKind::Requirement).len(),
        store.load_lenient(EntryKind::Source).len(),
    )
}

fn count_graph_checks(layout: &LayoutContext) -> usize {
    if !layout.domain_toml_path.is_file() {
        return 0;
    }
    match DomainConfig::load(&layout.domain_toml_path) {
        Ok(dom) => dom.module.graph_checks.len() + dom.project.graph_checks.len(),
        Err(_) => 0,
    }
}

/// Manifest-driven dispatch for the `module.schema.yaml`
/// / `requirements.schema.yaml` missing-warning suppressions. Returns
/// `(suppress_module, suppress_requirements)`.
///
/// Manifest path: when `<specdev_root>/profile.yaml` is present,
/// inspect `manifest.surface.has_modules` and `manifest.surface.has_requirements`.
/// Each schema is independently gated; today every shipped profile happens
/// to have `has_requirements` mirror `has_modules`, but the schema admits
/// asymmetric configurations (a profile could declare `has_modules: false`
/// + `has_requirements: true`) and the dispatch wires the granularity now.
///
/// Backward-compat fallback: when `profile.yaml` is absent
/// OR `manifest.surface` is None, fall back to the legacy
/// `layout.spec.workspace.is_some()` heuristic for BOTH suppressions.
/// Preserves pre-manifest fixture behavior verbatim. Removed in v1.1.
///
/// Malformed-manifest path: `MANIFEST-001` finding pushed; fall back to
/// the legacy heuristic so we don't surface a "missing schema" warning
/// stacked on the manifest-load error. The suppression site emits no
/// finding of its own (it's purely a control-flow decision).
fn compute_surface_suppressions(layout: &LayoutContext, report: &mut Report) -> (bool, bool) {
    use crate::manifest::Profile;
    let manifest_path = layout.specdev_root.join("profile.yaml");
    if !manifest_path.is_file() {
        let legacy = layout.spec.workspace.is_some();
        return (legacy, legacy);
    }
    match Profile::load(&manifest_path) {
        Ok(manifest) => match manifest.surface.as_ref() {
            Some(surface) => (!surface.has_modules, !surface.has_requirements),
            None => {
                let legacy = layout.spec.workspace.is_some();
                (legacy, legacy)
            }
        },
        Err(e) => {
            report.push(
                Finding::new("MANIFEST-001", Severity::Error, format!("{e:#}"))
                    .with_file(manifest_path.display().to_string()),
            );
            let legacy = layout.spec.workspace.is_some();
            (legacy, legacy)
        }
    }
}
