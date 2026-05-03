//! Validate-pipeline orchestrator (Phase 2 of the blocks + bridges + profiles arc).
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
use crate::layout::{LayoutContext, LayoutKind};
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
    validate_layout(layout, &mut report)?;
    validate_modules(layout, &mut report)?;
    validate_architecture(layout, &mut report)?;
    validate_requirements(layout, &mut report)?;
    Ok(report)
}

/// Schema-validate `specs/architecture.toml` against
/// `specs/specdev/schemas/architecture.schema.yaml`. Findings emitted via
/// `SCHEMA-ARCH`. Phase 5+.
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
/// `SCHEMA-REQ`. Phase 5+.
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
                format!("{label}.toml: {err}"),
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

    // Embedded, project-independent schemas (Phase 6.7+). Registered
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
    for entry in &inv.directories {
        if entry.exists {
            report.push(Finding::new(
                "LAYOUT-002",
                Severity::Info,
                format!("present: {}", entry.label),
            ));
        } else {
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
            // Skip the missing-warning for the codegen <lang> directory: it
            // is profile-driven and the project may not have authored
            // codegen yet.
            if entry.kind == LayoutKind::Directory
                && entry.label.starts_with("specs/specdev/codegen/")
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

/// Phase 2 module pass.
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

    // Step 1: build effective kind set.
    let raw_inv = reductions::load_all(layout)?;
    let (kindset, kindset_findings) = reductions::resolve(&raw_inv);
    report.extend(kindset_findings);

    // Step 2: compile aggregator schema.
    let validator = aggregator::compile(layout, report)?;

    // Step 3: validate each module TOML.
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
                    Finding::new("SCHEMA", Severity::Error, format!("{err}"))
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
    }

    // Step 4: domain.toml graph-check resolution + execution.
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

                // Step 5: run executor + project graph_checks.
                let mut exec_report = Report::default();
                let graph =
                    reduction::execute_kindset(&kindset, &layout.project_root, &mut exec_report)?;
                report.extend(exec_report.findings().to_vec());

                execute_graph_checks(
                    &dom.project.graph_checks,
                    "project",
                    &kindset,
                    &graph,
                    &dom,
                    layout,
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

/// Run domain.toml graph_checks against the executor's project graph.
///
/// The executor's [`Graph`] is serialized to a JSON value and walked via
/// the existing selector primitives in [`crate::graph`]. Two Phase-2
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
fn execute_graph_checks(
    checks: &[GraphCheck],
    section: &str,
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
        let severity = domain.severity_of(rule);
        let primitive = check.kind.as_str();

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
                (Some(ef), Some(et)) => graph_checks::acyclic(&view, ef, et, rule, severity),
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
            "requirements_have_satisfiers" => {
                match (check.entity_scope.as_deref(), check.relation_scope.as_deref()) {
                    (Some(es), Some(rs)) => {
                        let view =
                            filter_for_layer_dependency(&graph_json, es, rs, kindset);
                        graph_checks::requirements_have_satisfiers(&view, rule, severity)
                    }
                    _ => Vec::new(),
                }
            }
            // D37 / Phase 10: source-aware check. Each
            // `requirement.verified_by[]` entry resolves against tree-sitter-
            // extracted top-level items + #[test] items per module. Unlike
            // the other primitives, this one reads filesystem source via
            // the items extraction surface (Phase 10 ships the surface;
            // Phase 14 promotes the warning to error when items[] is the
            // authoritative substrate).
            "verified_by_resolves" => verified_by_resolves_dispatch(
                &graph_json,
                layout,
                rule,
                severity,
            ),
            other => vec![Finding::new(
                "DOMAIN-003",
                Severity::Warning,
                format!(
                    "domain.toml [{section}] graph_check (rule `{rule}`): \
                     unknown primitive `{other}` — supported: \
                     unique, reference_exists, reachability, acyclic, \
                     layer_dependency, requirements_have_satisfiers, verified_by_resolves"
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
fn graph_json_full<'a>(
    full: &'a JsonValue,
    _scope_filtered: &'a JsonValue,
) -> &'a JsonValue {
    full
}

/// Dispatch for the layer_dependency primitive.
///
/// Two execution modes coexist:
///
/// 1. **Graph-shape mode (Phase 5+, preferred):** `entity_scope` +
///    `relation_scope` + `layer_field` + `layer_ordering`. The graph is
///    jointly filtered (entities of the entity kind, relations of the
///    relation kind) and [`graph_checks::layer_dependency_graph`] is
///    invoked.
/// 2. **Legacy mode:** `module_layer_field` + `module_deps_field` +
///    `[layers.allowed_deps]` table. Source-data shape; retained for
///    pre-Phase-5 callers and the existing unit test.
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
        let view =
            filter_for_layer_dependency(full_graph, entity_scope, relation_scope, kindset);
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

/// D37 / Phase 10 — `verified_by_resolves` dispatch.
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
/// Severity: warning in v1.0; promotes to error in v1.1 alongside Phase
/// 14 items[] (per D37). Error code: `VERIFIED-BY-RESOLVES-001`.
fn verified_by_resolves_dispatch(
    graph_json: &JsonValue,
    layout: &LayoutContext,
    rule: &str,
    severity: Severity,
) -> Vec<Finding> {
    use crate::items::{extract as items_extract, source_map, ExtractedItem, ItemKind};
    use crate::lang;

    let language = &layout.spec.project.language;
    let Some(adapter) = lang::adapter_for(language) else {
        // Unsupported language — silently skip (matches the dispatch's
        // permissive behaviour for unknown shapes).
        return Vec::new();
    };
    let source_root = layout.spec.paths.source.as_str();
    let project_root = &layout.project_root;

    // Cache of module name → extracted items. Avoid re-parsing.
    let mut items_by_module: HashMap<String, Vec<ExtractedItem>> = HashMap::new();

    // Helper to lazily fetch items for a module.
    let mut get_items = |module_name: &str| -> Vec<ExtractedItem> {
        if let Some(cached) = items_by_module.get(module_name) {
            return cached.clone();
        }
        let items = match source_map::resolve_source_path(
            module_name,
            adapter.as_ref(),
            project_root,
            source_root,
        ) {
            Some(p) => items_extract::extract_items_from_path(&p, adapter.as_ref())
                .unwrap_or_default(),
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
        let Some(kind) = ent.get("kind").and_then(|k| k.as_str()) else { continue };
        if !kind.ends_with("Module") && kind != "Module" {
            continue;
        }
        if let Some(n) = ent.get("name").and_then(|n| n.as_str()) {
            all_module_names.push(n.to_string());
        }
    }

    let mut findings = Vec::new();

    for ent in entities {
        let Some(kind) = ent.get("kind").and_then(|k| k.as_str()) else { continue };
        if !kind.ends_with("Requirement") && kind != "Requirement" {
            continue;
        }
        let Some(req_id) = ent.get("name").and_then(|n| n.as_str()) else { continue };
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
                    // Pre-implementation module: skip (silent), per D37.
                    continue;
                }
                items.iter().any(|i| i.name == name)
            } else {
                // Bare name: search for a #[test]-attributed fn across all
                // modules. If no module has source yet, skip (silent).
                let mut any_source = false;
                let mut found = false;
                for m in &all_module_names {
                    let items = get_items(m);
                    if !items.is_empty() {
                        any_source = true;
                    }
                    if items.iter().any(|i| {
                        i.kind == ItemKind::Fn
                            && i.name == s
                            && i.attributes.iter().any(|a| a.contains("test"))
                    }) {
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
fn filter_for_layer_dependency(
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
            if let Some(entities) =
                filtered.get_mut("entities").and_then(|v| v.as_array_mut())
            {
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
            if let Some(relations) =
                filtered.get_mut("relations").and_then(|v| v.as_array_mut())
            {
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
    } else if let Some(entities) =
        filtered.get_mut("entities").and_then(|v| v.as_array_mut())
    {
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
/// fields. Phase 2's load-time resolution takes whichever fields hold
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
}

impl Summary {
    /// One-line rendering for the CLI tail.
    pub fn render(&self) -> String {
        format!(
            "loaded {} modules / {} requirements / {} sources / {} graph checks",
            self.modules, self.requirements, self.sources, self.graph_checks,
        )
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

    Summary {
        modules,
        requirements,
        sources,
        graph_checks,
    }
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
