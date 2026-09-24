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

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use jsonschema::{Retrieve, Uri};
use serde_json::Value as JsonValue;
use tracing::{info, instrument};

use crate::aggregator;
use crate::domain::{DomainConfig, GraphCheck, Severity};
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

/// Capability tag carried on each registered check.
/// Used by `validate --for=<capability>` to scope the checks that fire to
/// the slice the caller cares about; absent the flag every check runs.
///
/// The set is closed: each capability names a downstream consumer of the
/// validated spec. Adding a new capability is a one-line edit here +
/// adding the tag to whichever checks gate that consumer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Schema / structural integrity — every check here gates "this spec
    /// is well-formed". Default-on for every other capability since every
    /// downstream consumer needs schema-clean inputs.
    Schema,
    /// `gen --strict` (Claude-orchestrated codegen) consumes this. Strict
    /// mode is high-context: a module that omits `context_files` in
    /// strict mode gets no out-of-band prose, which is usually a friction
    /// (F-015 from the zfm dogfood).
    StrictGen,
    /// `cargo-toml` emission consumes this. Reserved; no checks are
    /// currently tagged `CargoToml` (the cargo-toml emitter today reads
    /// architecture.toml directly).
    #[allow(dead_code)]
    CargoToml,
}

impl std::str::FromStr for Capability {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "schema" => Ok(Capability::Schema),
            "strict-gen" => Ok(Capability::StrictGen),
            "cargo-toml" => Ok(Capability::CargoToml),
            other => Err(format!(
                "unknown capability `{other}`; supported: schema, strict-gen, cargo-toml"
            )),
        }
    }
}

/// Returns true when `tags` should run under the active `filter`. Absent
/// filter (None) → run all. Capability::Schema tags always run regardless
/// of filter — well-formedness is a load-bearing precondition for every
/// downstream consumer, so suppressing it would lie about the spec's state.
fn capability_matches(tags: &[Capability], filter: Option<Capability>) -> bool {
    let Some(want) = filter else { return true };
    if tags.contains(&Capability::Schema) {
        return true;
    }
    tags.contains(&want)
}

/// Library-friendly entry point. Discover `spec.toml` from `start` (or from
/// `explicit_spec_path` if set) and run validation end-to-end.
pub fn run_project(start: &Path, explicit_spec_path: Option<&Path>) -> Result<Report> {
    let layout = LayoutContext::discover(start, explicit_spec_path)?;
    run(&layout)
}

/// Capability-filtered variant: filters which checks fire by `Capability`.
/// `None` runs every check (= legacy `run_project` behavior). `Some(cap)`
/// runs only checks tagged with that capability or with `Schema`
/// (well-formedness is universal).
pub fn run_project_for(
    start: &Path,
    explicit_spec_path: Option<&Path>,
    filter: Option<Capability>,
) -> Result<Report> {
    let layout = LayoutContext::discover(start, explicit_spec_path)?;
    run_for(&layout, filter)
}

#[instrument(level = "info", skip(layout), fields(project = %layout.project_root.display()))]
pub fn run(layout: &LayoutContext) -> Result<Report> {
    run_for(layout, None)
}

/// Capability-filtered variant: see [`run_project_for`].
pub fn run_for(layout: &LayoutContext, filter: Option<Capability>) -> Result<Report> {
    let mut report = Report::default();
    validate_layout(layout, &mut report)?;
    validate_modules_for(layout, filter, &mut report)?;
    validate_architecture(layout, &mut report)?;
    validate_requirements(layout, &mut report)?;
    validate_items_present(layout, filter, &mut report);
    validate_reserved_test_kinds(layout, filter, &mut report);
    validate_context_files_when_strict(layout, filter, &mut report);
    validate_profile_layout_match(layout, &mut report);
    validate_profiles_root_only(layout, &mut report)?;
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
        "LAYOUT-005",
        report,
    )
}

/// Schema-validate `specs/requirements.toml` against
/// `specs/specdev/schemas/requirements.schema.yaml`. Findings emitted via
/// `SCHEMA-REQ`.
#[instrument(level = "info", skip(layout, report))]
pub fn validate_requirements(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    validate_aux_toml(
        layout,
        &layout.requirements_schema_path,
        &layout.requirements_toml_path,
        "requirements",
        "SCHEMA-REQ",
        "LAYOUT-006",
        report,
    )
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
    absent_rule: &str,
    report: &mut Report,
) -> Result<()> {
    if !schema_path.is_file() {
        // Already flagged by `validate_layout`'s file_entries pass; skip
        // here so we don't double-report.
        return Ok(());
    }
    if !toml_path.is_file() {
        report.push(Finding::new(
            absent_rule,
            Severity::Info,
            format!(
                "{label}.toml absent at {} — no {label}-level entries authored yet",
                toml_path.display()
            ),
        ));
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
                    format!("{label}.toml is not valid TOML: {e}"),
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
                    "{label}.toml: {}",
                    crate::diagnostics::format_schema_error(&err)
                ),
            )
            .with_file(toml_path.display().to_string())
            .with_pointer(err.instance_path().to_string()),
        );
    }

    Ok(())
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
    // passes continue (validate_profile_layout_match also surfaces
    // MANIFEST-001 for the same file — duplicate-rule emission is
    // tolerated; deduping is a v1.x cleanup).
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
            // Skip the missing-warning for the codegen <lang> directory:
            // it is profile-driven and the project may not have authored
            // codegen yet. (Intent originally lived in the file loop, where
            // it was dead code since codegen/<lang>/ is a directory entry.)
            if entry.label.starts_with("specs/specdev/codegen/") {
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
    if layout.codegen_dir.is_dir() {
        report.push(Finding::new(
            "LAYOUT-004",
            Severity::Info,
            format!(
                "codegen directory present for language `{}` at {}",
                layout.spec.project.language,
                layout.codegen_dir.display()
            ),
        ));
    } else {
        report.push(Finding::new(
            "LAYOUT-004",
            Severity::Warning,
            format!(
                "codegen directory absent for language `{}` (expected at {})",
                layout.spec.project.language,
                layout.codegen_dir.display()
            ),
        ));
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
    validate_modules_for(layout, None, report)
}

/// Filter-aware variant. See [`Capability`] and [`run_for`].
pub fn validate_modules_for(
    layout: &LayoutContext,
    filter: Option<Capability>,
    report: &mut Report,
) -> Result<()> {
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
                let graph = reduction::execute_kindset(&kindset, layout, &mut exec_report)?;
                report.extend(exec_report.findings().to_vec());

                execute_graph_checks(
                    &dom.project.graph_checks,
                    "project",
                    &kindset,
                    &graph,
                    &dom,
                    layout,
                    filter,
                    report,
                );
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
    let context_files = crate::prompt::parse_context_files(parsed);
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
    kindset: &EffectiveKindSet,
    graph: &Graph,
    domain: &DomainConfig,
    layout: &LayoutContext,
    filter: Option<Capability>,
    report: &mut Report,
) {
    let graph_json = match serde_json::to_value(graph) {
        Ok(v) => v,
        Err(_) => return,
    };

    for check in checks {
        let rule = check.rule.as_deref().unwrap_or("?");
        let severity = domain.severity_of(rule);
        let primitive = check.kind.as_str();

        // Skip primitives whose tags don't match the active --for filter.
        // `Schema`-tagged checks always run (they
        // gate every downstream consumer).
        if !capability_matches(primitive_tags(primitive), filter) {
            continue;
        }

        let view = filter_graph_for_check(&graph_json, check, kindset);

        let findings = match primitive {
            "unique" => check
                .field
                .as_deref()
                .map(|f| graph_checks::unique(&view, f, rule, severity))
                .unwrap_or_default(),
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
            "verified_by_resolves" => {
                verified_by_resolves_dispatch(&graph_json, layout, rule, severity)
            }
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
            "module_reachable_through_open_question" => {
                graph_checks::module_reachable_through_open_question(&graph_json, rule, severity)
            }
            other => vec![Finding::new(
                "DOMAIN-003",
                Severity::Warning,
                format!(
                    "domain.toml [{section}] graph_check (rule `{rule}`): \
                     unknown primitive `{other}` — supported: \
                     unique, reference_exists, reachability, acyclic, \
                     layer_dependency, requirements_have_satisfiers, \
                     entity_has_inbound_relation, verified_by_resolves, \
                     entity_attr_inbound_count, entity_attr_target_attr_equals, \
                     module_reachable_through_open_question"
                ),
            )],
        };

        report.extend(findings);
    }
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

/// `verified_by_resolves` dispatch.
///
/// For each `Requirement` entity carrying a `verified_by[]` attr, parse
/// each entry and attempt resolution against the per-module item
/// extraction surface (`items::extract`). Two entry shapes:
///
/// - `<Module>::<name>` — resolves against the named module's source.
///   The matching item must be a `fn` with `#[test]` attribute (a test
///   function), or any top-level item with a matching name (forgiving:
///   non-test items match too, since the items[] design treats
///   `verified_by` as resolution against item names rather than a
///   `#[test]` filter).
/// - bare `<name>` — resolves against any module's `#[test]`-attributed
///   `fn` items.
///
/// Pre-implementation modules (no source file resolved) are silent: the
/// per-module item set is empty, but if every named module has no
/// source, the requirement entry is treated as "no candidates" and no
/// finding fires (avoids a wall of warnings for greenfield specs).
///
/// Severity: warning in v1.0; promotes to error in v1.1 alongside the
/// items[] surface. Error code: `VERIFIED-BY-RESOLVES-001`.
fn verified_by_resolves_dispatch(
    graph_json: &JsonValue,
    layout: &LayoutContext,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    use crate::items::{extract as items_extract, source_map, ExtractedItem};
    use crate::lang;

    let language = &layout.spec.project.language;
    let Some(adapter) = lang::adapter_for(language) else {
        // Unsupported language — silently skip (matches the dispatch's
        // permissive behaviour for unknown shapes).
        return Vec::new();
    };
    let source_root = layout.spec.paths.source.as_str();
    let project_root = &layout.project_root;
    // #101(b): profile floor (a malformed manifest is MANIFEST-001's
    // business elsewhere; here it means "adapter default", matching this
    // pass's permissive stance).
    let floor = items_extract::profile_visibility_floor(&layout.specdev_root)
        .ok()
        .flatten();

    // Cache of module name → extracted items. Avoid re-parsing.
    let mut items_by_module: HashMap<String, Vec<ExtractedItem>> = HashMap::new();

    // Helper to lazily fetch items for a module.
    let mut get_items = |module_name: &str| -> Vec<ExtractedItem> {
        if let Some(cached) = items_by_module.get(module_name) {
            return cached.clone();
        }
        // #101(a): honour the module spec's `source_file` override.
        let module_path = layout.modules_dir.join(format!("{module_name}.toml"));
        let override_path = source_map::load_source_file_override(&module_path);
        let items = match source_map::resolve_source_path(
            module_name,
            override_path.as_deref(),
            adapter.as_ref(),
            project_root,
            source_root,
        ) {
            Some(p) => {
                items_extract::extract_items_from_path(&p, adapter.as_ref(), floor.as_deref())
                    .unwrap_or_default()
            }
            None => Vec::new(),
        };
        items_by_module.insert(module_name.to_string(), items.clone());
        items
    };

    let entities = match graph_json.get("entities").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return Vec::new(),
    };

    // Collect all module names referenced by Requirement entities so we
    // can pre-populate the items cache and detect bare-name resolution
    // candidates across modules.
    let mut all_module_names: Vec<String> = Vec::new();
    for ent in entities {
        let Some(kind) = ent.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        if !kind.ends_with("Module") && kind != "Module" {
            continue;
        }
        if let Some(n) = ent.get("name").and_then(|n| n.as_str()) {
            all_module_names.push(n.to_string());
        }
    }

    let mut findings = Vec::new();

    for ent in entities {
        let Some(kind) = ent.get("kind").and_then(|k| k.as_str()) else {
            continue;
        };
        if !kind.ends_with("Requirement") && kind != "Requirement" {
            continue;
        }
        let Some(req_id) = ent.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let Some(verified_by) = ent
            .get("attrs")
            .and_then(|a| a.get("verified_by"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        for entry in verified_by {
            let Some(s) = entry.as_str() else { continue };
            let s = s.trim();
            if s.is_empty() {
                continue;
            }
            // Module::name vs bare name.
            let resolved = if let Some((module, name)) = s.split_once("::") {
                let module = module.trim();
                let name = name.trim();
                let items = get_items(module);
                if items.is_empty() {
                    // Pre-implementation module: skip (silent).
                    continue;
                }
                items.iter().any(|i| i.name == name)
            } else {
                // Bare name: search for a test function across all modules
                // (what counts as one is the adapter's rule — Rust: a fn
                // carrying a test attribute). If no module has source yet,
                // skip (silent).
                let mut any_source = false;
                let mut found = false;
                for m in &all_module_names {
                    let items = get_items(m);
                    if !items.is_empty() {
                        any_source = true;
                    }
                    if items
                        .iter()
                        .any(|i| i.name == s && adapter.is_test_function(i))
                    {
                        found = true;
                        break;
                    }
                }
                if !any_source {
                    continue;
                }
                found
            };

            if !resolved {
                findings.push(Finding::new(
                    rule,
                    severity,
                    format!(
                        "[VERIFIED-BY-RESOLVES-001] requirement `{req_id}` \
                         verified_by entry `{s}` does not resolve to any \
                         extracted top-level item; check spelling, the named \
                         module's source, or remove the entry"
                    ),
                ));
            }
        }
    }

    findings
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
    for (idx, check) in checks.iter().enumerate() {
        let candidates: [&Option<String>; 7] = [
            &check.scope,
            &check.field,
            &check.source_field,
            &check.target_field,
            &check.nodes_field,
            &check.edges_from,
            &check.edges_to,
        ];
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
                            "domain.toml [{section}] graph_check #{idx} (rule `{}`): \
                             kind reference `{candidate}` is not in the effective kind set",
                            check.rule.as_deref().unwrap_or("?"),
                        ),
                    )
                    .with_file(domain_toml_path.display().to_string()),
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
    let decisions = count_toml_array(&layout.specs_dir.join("decisions.toml"), "decision");
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

/// Best-effort count of a top-level array-of-tables in a TOML file.
/// Missing or unparseable files count as zero, mirroring
/// [`count_requirements_and_sources`].
fn count_toml_array(path: &std::path::Path, key: &str) -> usize {
    if !path.is_file() {
        return 0;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Ok(parsed): std::result::Result<toml::Value, _> = toml::from_str(&text) else {
        return 0;
    };
    parsed
        .get(key)
        .and_then(toml::Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Best-effort (total, open) counts over `specs/open_questions.toml`.
fn count_open_questions(layout: &LayoutContext) -> (usize, usize) {
    let path = layout.specs_dir.join("open_questions.toml");
    if !path.is_file() {
        return (0, 0);
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return (0, 0);
    };
    let Ok(parsed): std::result::Result<toml::Value, _> = toml::from_str(&text) else {
        return (0, 0);
    };
    let Some(arr) = parsed.get("open_question").and_then(toml::Value::as_array) else {
        return (0, 0);
    };
    let open = arr
        .iter()
        .filter(|q| q.get("status").and_then(toml::Value::as_str) == Some("open"))
        .count();
    (arr.len(), open)
}

/// Capability tags carried by each registered graph-check primitive.
/// Driven by `validate --for=<capability>`. Tags
/// are intentionally redundant — every primitive tags `Schema` because
/// well-formedness gates every downstream consumer; some additionally
/// tag domain-specific consumers (`Skeleton`, `StrictGen`, etc.).
///
/// Adding a new primitive: append a match arm AND a stable test that
/// verifies its tag set. Adding a new capability: extend the
/// [`Capability`] enum and tag the primitives that gate it.
fn primitive_tags(name: &str) -> &'static [Capability] {
    use Capability::*;
    match name {
        "unique" | "reference_exists" | "reachability" | "acyclic" => &[Schema],
        "layer_dependency" => &[Schema],
        // Orphan-requirements is a structural check on the spec graph;
        // strict-gen consumes the spec, so an unsatisfied requirement
        // means an orphan story regardless of how codegen runs.
        "requirements_have_satisfiers" => &[Schema, StrictGen],
        // Generic orphan check; structural on the spec graph,
        // and strict-gen consumes the spec, so an orphan entity (e.g. a
        // parameter with no engine binding) is a gap regardless of how
        // codegen runs.
        "entity_has_inbound_relation" => &[Schema, StrictGen],
        // verified_by_resolves reads source files to validate that
        // requirement.verified_by[] entries name extant items / tests.
        // Strict-gen will surface a verification gap.
        "verified_by_resolves" => &[Schema, StrictGen],
        // Triad consistency primitives gate codegen via the
        // companion gate, so tag them Schema (always-on) and StrictGen.
        "entity_attr_inbound_count"
        | "entity_attr_target_attr_equals"
        | "module_reachable_through_open_question" => &[Schema, StrictGen],
        // Unknown primitives still surface their DOMAIN-003 warning;
        // tag them as Schema so the typo isn't silenced by --for.
        _ => &[Schema],
    }
}

/// F-013 (zfm dogfood): warns when a module has zero
/// `items[]` entries. Strict mode has no items[] to anchor codegen to.
/// Info-level: not every module needs items (a project mid-port may
/// have empty modules), but every empty module deserves visibility.
///
/// Tag: [`Capability::StrictGen`]. Skipped when `--for=cargo-toml` is
/// active (cargo-toml emission doesn't consume items).
pub fn validate_items_present(
    layout: &LayoutContext,
    filter: Option<Capability>,
    report: &mut Report,
) {
    let tags: &[Capability] = &[Capability::StrictGen];
    if !capability_matches(tags, filter) {
        return;
    }
    let inv = layout.enumerate();
    for module_path in &inv.modules {
        let Ok(text) = std::fs::read_to_string(module_path) else {
            continue;
        };
        let Ok(parsed) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        let name = parsed
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| {
                module_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("<unnamed>")
            });
        let items_count = parsed
            .get("items")
            .and_then(toml::Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        if items_count == 0 {
            report.push(
                Finding::new(
                    "ITEMS-001",
                    Severity::Info,
                    format!(
                        "module `{name}` has 0 items[] entries — `gen` has nothing to \
                         anchor codegen to. Add items[] before running codegen. Every \
                         ordinary item also needs `satisfies = [\"REQ-...\"]` (≥1 id); \
                         only a test item (`test = true`, which carries `verifies`) or \
                         a re-export is exempt."
                    ),
                )
                .with_file(module_path.display().to_string()),
            );
        }
    }
}

/// Warns when a test item declares a `test_kind` whose
/// emission is reserved-but-not-shipped. The reserved list is language
/// data, not analyzer code: the language tier's `items.schema.yaml`
/// declares it as `properties.test_kind.x-specdev-reserved` next to the
/// `test_kind` enum (the schema accepts the enum slot so later promotion
/// needs no spec migration, but `gen` emits nothing for it — surfacing
/// the gap at validate time keeps authors from being surprised at
/// codegen). A language tier without the annotation reserves nothing;
/// a project without an inlined items tier has no test items to check
/// (the module-schema `$ref` would already have failed validation).
///
/// Tag: [`Capability::StrictGen`] — the gap only matters when codegen
/// actually runs.
pub fn validate_reserved_test_kinds(
    layout: &LayoutContext,
    filter: Option<Capability>,
    report: &mut Report,
) {
    let tags: &[Capability] = &[Capability::StrictGen];
    if !capability_matches(tags, filter) {
        return;
    }
    let Some((reserved, emittable)) = load_test_kind_vocab(layout) else {
        return;
    };
    if reserved.is_empty() {
        return;
    }
    let advice = emittable
        .iter()
        .map(|k| format!("`{k}`"))
        .collect::<Vec<_>>()
        .join(" / ");
    let inv = layout.enumerate();
    for module_path in &inv.modules {
        let Ok(text) = std::fs::read_to_string(module_path) else {
            continue;
        };
        let Ok(parsed) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        let Some(items) = parsed.get("items").and_then(toml::Value::as_array) else {
            continue;
        };
        for item in items {
            if item.get("test").and_then(toml::Value::as_bool) != Some(true) {
                continue;
            }
            let Some(kind) = item.get("test_kind").and_then(toml::Value::as_str) else {
                continue;
            };
            if reserved.iter().any(|r| r == kind) {
                let name = item
                    .get("name")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("<unnamed>");
                report.push(
                    Finding::new(
                        "TEST-KIND-001",
                        Severity::Warning,
                        format!(
                            "test item `{name}` has test_kind = \"{kind}\", which reserves \
                             the enum slot but emits no body in v1 — `gen` will skip it. \
                             Use {advice}, or hand-author the \
                             {kind} harness until v1.x ships emission."
                        ),
                    )
                    .with_file(module_path.display().to_string()),
                );
            }
        }
    }
}

/// Read the language tier's `test_kind` vocabulary from the project's
/// inlined `languages/<lang>/items.schema.yaml`: the reserved list
/// (`x-specdev-reserved`, defaulting to empty) and the emittable
/// remainder of the enum (schema order), for the TEST-KIND-001 advice
/// text. `None` when the tier file is absent or unparsable — there is
/// then no test_kind vocabulary to lint against (and a project actually
/// using items[] surfaces the missing tier through schema resolution).
fn load_test_kind_vocab(layout: &LayoutContext) -> Option<(Vec<String>, Vec<String>)> {
    let path = layout
        .inlined_languages_dir
        .join(&layout.spec.project.language)
        .join("items.schema.yaml");
    let text = std::fs::read_to_string(&path).ok()?;
    let doc: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    let test_kind = doc.get("properties")?.get("test_kind")?;
    let as_strings = |v: Option<&serde_yaml::Value>| -> Vec<String> {
        v.and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let reserved = as_strings(test_kind.get("x-specdev-reserved"));
    let emittable: Vec<String> = as_strings(test_kind.get("enum"))
        .into_iter()
        .filter(|k| !reserved.iter().any(|r| r == k))
        .collect();
    Some((reserved, emittable))
}

/// F-015 (zfm dogfood): warns when a module has no `context_files`
/// entries. The dogfood reported `module.context_files` as
/// undiscoverable from the `/spec-design` interview; surfacing the gap
/// at validate time gives the user a structural prompt to add prose
/// context (or to leave the module shallow enough to need none).
///
/// Tag: [`Capability::StrictGen`]. Skipped when `--for=cargo-toml` is
/// active. Warning-level — not every module needs context files
/// (mechanical glue modules legitimately don't), but the absence is
/// worth surfacing for shipped strict-gen runs.
pub fn validate_context_files_when_strict(
    layout: &LayoutContext,
    filter: Option<Capability>,
    report: &mut Report,
) {
    let tags: &[Capability] = &[Capability::StrictGen];
    if !capability_matches(tags, filter) {
        return;
    }
    let inv = layout.enumerate();
    for module_path in &inv.modules {
        let Ok(text) = std::fs::read_to_string(module_path) else {
            continue;
        };
        let Ok(parsed) = toml::from_str::<toml::Value>(&text) else {
            continue;
        };
        let name = parsed
            .get("name")
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| {
                module_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("<unnamed>")
            });
        let has_context = parsed
            .get("context_files")
            .and_then(toml::Value::as_array)
            .is_some_and(|a| !a.is_empty());
        if !has_context {
            report.push(
                Finding::new(
                    "CONTEXT-100",
                    Severity::Warning,
                    format!(
                        "module `{name}` has no `context_files`. Strict codegen has no \
                         out-of-band prose to anchor on; consider adding \
                         `context_files = [\"docs/<...>.md\"]`."
                    ),
                )
                .with_file(module_path.display().to_string()),
            );
        }
    }
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
    if !layout.requirements_toml_path.is_file() {
        return (0, 0);
    }
    let Ok(text) = std::fs::read_to_string(&layout.requirements_toml_path) else {
        return (0, 0);
    };
    let Ok(parsed): std::result::Result<toml::Value, _> = toml::from_str(&text) else {
        return (0, 0);
    };
    let reqs = parsed
        .get("requirement")
        .and_then(toml::Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let srcs = parsed
        .get("source")
        .and_then(toml::Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    (reqs, srcs)
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

/// Validate-time consistency check between
/// the inlined `profile.yaml` (`cargo_toml.shape`) and the project's
/// spec.toml `[workspace]` table presence.
///
/// `shape: workspace` is descriptive metadata: a profile claims its
/// projects are workspace-shaped, but actual workspace dispatch is
/// layout-driven (the `[workspace]` table in spec.toml is canonical).
/// When the two disagree, the project is in an unrenderable
/// state — `cargo-toml render` will eventually error, but surfacing
/// the mismatch at validate time gives the user a structural prompt
/// before they hit the render path.
///
/// No-op when `profile.yaml` is absent (backward compat) or
/// has no `cargo_toml` section. Manifest load errors surface as
/// `MANIFEST-001` findings; cargo-toml render will produce a
/// matching diagnostic, so consumers see the same root cause from
/// either path.
pub fn validate_profile_layout_match(layout: &LayoutContext, report: &mut Report) {
    use crate::manifest::{Profile, ShapeKind};
    let manifest_path = layout.specdev_root.join("profile.yaml");
    if !manifest_path.is_file() {
        return;
    }
    let manifest = match Profile::load(&manifest_path) {
        Ok(p) => p,
        Err(e) => {
            report.push(
                Finding::new("MANIFEST-001", Severity::Error, format!("{e:#}"))
                    .with_file(manifest_path.display().to_string()),
            );
            return;
        }
    };
    let Some(cargo) = manifest.cargo_toml.as_ref() else {
        return;
    };
    let has_workspace_table = layout.spec.workspace.is_some();
    let is_workspace_shape = matches!(cargo.shape, ShapeKind::Workspace);
    match (is_workspace_shape, has_workspace_table) {
        (true, false) => report.push(
            Finding::new(
                "PROFILE-LAYOUT-MISMATCH",
                Severity::Error,
                format!(
                    "profile.yaml declares `cargo_toml.shape: workspace` but \
                     spec.toml at {} has no [workspace] table; add \
                     [workspace] with [[workspace.members]] entries or \
                     change the profile",
                    layout.spec_toml_path.display(),
                ),
            )
            .with_file(manifest_path.display().to_string()),
        ),
        (false, true) => report.push(
            Finding::new(
                "PROFILE-LAYOUT-MISMATCH",
                Severity::Error,
                format!(
                    "spec.toml at {} declares a [workspace] table but \
                     profile.yaml `cargo_toml.shape` is non-workspace; \
                     workspace projects must use a profile with \
                     `cargo_toml.shape: workspace`",
                    layout.spec_toml_path.display(),
                ),
            )
            .with_file(manifest_path.display().to_string()),
        ),
        _ => {}
    }
}

/// Root-only rule for Cargo build profiles: `architecture.toml`'s
/// `profiles` table is legal on a workspace root or a standalone crate,
/// never on a workspace member. Cargo ignores profiles in a member
/// manifest (with a warning nobody reads), so a member that declares
/// them carries build configuration that silently does nothing. Finding
/// `PROFILES-ROOT-ONLY` names the workspace root as the home. The
/// cargo-toml renderer refuses the same case on the emit path, since
/// `manifest` does not run `validate`.
///
/// Membership is decided by the same walk the renderer uses
/// (`layout::find_workspace_root_spec`). A parse failure of
/// architecture.toml is left to the schema pass (`SCHEMA-ARCH`).
pub fn validate_profiles_root_only(layout: &LayoutContext, report: &mut Report) -> Result<()> {
    if layout.spec.workspace.is_some() {
        return Ok(());
    }
    if !layout.architecture_toml_path.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&layout.architecture_toml_path)
        .with_context(|| format!("reading {}", layout.architecture_toml_path.display()))?;
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return Ok(());
    };
    let declares = value
        .get("profiles")
        .and_then(toml::Value::as_table)
        .map(|t| !t.is_empty())
        .unwrap_or(false);
    if !declares {
        return Ok(());
    }
    let Some(root) = crate::layout::find_workspace_root_spec(layout.project_root.as_path())? else {
        return Ok(());
    };
    report.push(
        Finding::new(
            "PROFILES-ROOT-ONLY",
            Severity::Error,
            format!(
                "architecture.toml declares [profiles.*] but this crate is a member of the \
                 workspace at {}; Cargo ignores profiles in a member manifest — move them to \
                 that workspace's architecture.toml",
                root.display(),
            ),
        )
        .with_file(layout.architecture_toml_path.display().to_string()),
    );
    Ok(())
}
