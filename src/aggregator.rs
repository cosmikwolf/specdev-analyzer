//! Aggregator schema `$ref` resolver.
//!
//! Phase 1: load `specs/specdev/schemas/module.schema.yaml`, walk its
//! top-level `allOf` array, and resolve each `$ref` against the inlined
//! fragments under `inlined/blocks/` and `inlined/bridges/`. Validates that
//! every fragment carries a unique `$id` (the risk called out in the plan).
//!
//! Phase 1 deliberately does NOT compile the aggregated schema — that's
//! Phase 2's job once the metaschema amendment lands. Cycle detection is
//! warn-only here; cycles become fatal in Phase 2 when the schema is
//! actually compiled.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use jsonschema::{Retrieve, Uri, Validator};
use serde_json::Value;
use tracing::{event, instrument, Level};

use crate::diagnostics::LocatedError;
use crate::layout::LayoutContext;
use crate::report::{Finding, Report};

#[derive(Debug, Clone)]
pub struct AggregatorReport {
    pub aggregator_loaded: bool,
    pub fragments_loaded: usize,
    pub refs_total: usize,
    pub refs_resolved: usize,
    pub refs_unresolved: usize,
    pub duplicate_ids: usize,
    pub cycles_detected: usize,
}

/// Resolve the aggregator's `$ref`s and report findings. Findings are
/// merged into `report`.
#[instrument(level = "info", skip(layout, report), fields(aggregator = %layout.module_schema_path.display()))]
pub fn resolve(layout: &LayoutContext, report: &mut Report) -> Result<AggregatorReport> {
    let mut summary = AggregatorReport {
        aggregator_loaded: false,
        fragments_loaded: 0,
        refs_total: 0,
        refs_resolved: 0,
        refs_unresolved: 0,
        duplicate_ids: 0,
        cycles_detected: 0,
    };

    // Load fragments under inlined/blocks/ and inlined/bridges/. Each
    // fragment must carry a unique `$id` across the union; the duplicate
    // check runs before the aggregator walk.
    let fragments = load_fragments(layout, report)?;
    summary.fragments_loaded = fragments.len();

    // Aggregator schema may not exist yet on a skeleton project. Phase 1
    // emits a missing-file finding (LAYOUT-003) elsewhere; here we just
    // skip aggregator walking.
    if !layout.module_schema_path.is_file() {
        return Ok(summary);
    }
    summary.aggregator_loaded = true;

    let aggregator_text = std::fs::read_to_string(&layout.module_schema_path)
        .with_context(|| format!("reading {}", layout.module_schema_path.display()))?;
    let aggregator_value: Value = serde_yaml::from_str(&aggregator_text).map_err(|e| {
        anyhow::Error::new(LocatedError::from_yaml_error(
            &layout.module_schema_path,
            &e,
        ))
    })?;

    let refs = collect_all_of_refs(&aggregator_value);
    summary.refs_total = refs.len();

    for r in &refs {
        if let Some(_resolved) = resolve_ref(&r.value, &fragments) {
            summary.refs_resolved += 1;
            event!(
                Level::DEBUG,
                target = %r.value,
                "ref_resolved",
            );
        } else {
            summary.refs_unresolved += 1;
            event!(
                Level::WARN,
                target = %r.value,
                "ref_unresolved",
            );
            let loaded_names: Vec<String> = fragments.values().map(|f| f.label.clone()).collect();
            let msg = format!(
                "AGGREGATOR-001 unresolved $ref `{}` (loaded fragments: [{}])",
                r.value,
                loaded_names.join(", ")
            );
            report.push(
                Finding::new("AGGREGATOR-001", crate::domain::Severity::Error, msg)
                    .with_file(layout.module_schema_path.display().to_string()),
            );
        }
    }

    // Phase 2 promotes cycle detection from warn → error. Doc 10's
    // commitment: "Phase 2 will fail at compile time" — we now do.
    let cycles = detect_cycles(&fragments);
    summary.cycles_detected = cycles.len();
    for cycle in &cycles {
        let path = cycle.join(" → ");
        event!(Level::ERROR, cycle = %path, "cycle_detected");
        report.push(Finding::new(
            "AGGREGATOR-003",
            crate::domain::Severity::Error,
            format!("AGGREGATOR-003 cycle detected through inlined fragments: {path}"),
        ));
    }

    summary.duplicate_ids = report
        .findings()
        .iter()
        .filter(|f| f.rule == "AGGREGATOR-002")
        .count();

    Ok(summary)
}

#[derive(Debug, Clone)]
struct Fragment {
    /// Filename (basename) — what aggregator $refs typically use.
    label: String,
    #[allow(dead_code)]
    path: PathBuf,
    /// Declared `$id` if present in the YAML.
    id: Option<String>,
    /// Set of `$ref` strings this fragment itself contains (used for cycle
    /// detection in Phase 1; minimal walk since Phase 2 will replace this).
    refs: Vec<String>,
}

fn load_fragments(
    layout: &LayoutContext,
    report: &mut Report,
) -> Result<HashMap<String, Fragment>> {
    let mut out: HashMap<String, Fragment> = HashMap::new();
    // Track $id → originating filenames to detect duplicates across blocks
    // and bridges.
    let mut by_id: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();

    for dir in [&layout.inlined_blocks_dir, &layout.inlined_bridges_dir] {
        if !dir.is_dir() {
            continue;
        }
        let entries: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .filter_map(Result::ok)
            .collect();
        for entry in entries {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if !ext.eq_ignore_ascii_case("yaml") && !ext.eq_ignore_ascii_case("yml") {
                continue;
            }
            let label = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let value: Value = match serde_yaml::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    return Err(anyhow::Error::new(LocatedError::from_yaml_error(&path, &e)));
                }
            };
            let id = value
                .get("$id")
                .and_then(Value::as_str)
                .map(|s| s.to_string());
            if let Some(id) = &id {
                by_id.entry(id.clone()).or_default().push(path.clone());
            }
            let refs = collect_refs(&value);
            out.insert(
                label.clone(),
                Fragment {
                    label,
                    path,
                    id,
                    refs,
                },
            );
        }
    }

    // Duplicate-$id check. Emit one error per duplicated $id naming all
    // owning files.
    for (id, paths) in &by_id {
        if paths.len() > 1 {
            let listed: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            let msg = format!(
                "AGGREGATOR-002 duplicate $id `{id}` declared by multiple fragments: [{}]",
                listed.join(", ")
            );
            report.push(
                Finding::new("AGGREGATOR-002", crate::domain::Severity::Error, msg)
                    .with_file(paths[0].display().to_string()),
            );
        }
    }

    Ok(out)
}

#[derive(Debug, Clone)]
struct AggRef {
    value: String,
}

fn collect_all_of_refs(aggregator: &Value) -> Vec<AggRef> {
    let Some(all_of) = aggregator.get("allOf").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in all_of {
        if let Some(r) = item.get("$ref").and_then(Value::as_str) {
            out.push(AggRef {
                value: r.to_string(),
            });
        }
    }
    out
}

fn collect_refs(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(value, &mut out);
    out
}

fn walk(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "$ref" {
                    if let Some(s) = v.as_str() {
                        out.push(s.to_string());
                    }
                } else {
                    walk(v, out);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                walk(v, out);
            }
        }
        _ => {}
    }
}

fn resolve_ref<'a>(
    reference: &str,
    fragments: &'a HashMap<String, Fragment>,
) -> Option<&'a Fragment> {
    // Aggregator $refs typically look like:
    //   "inlined/blocks/embassy.schema.yaml"
    //   "inlined/bridges/embassy+lvgl.schema.yaml"
    //   "https://specdev.dev/blocks/embassy.v1"   (bare $id)
    let basename = reference.rsplit('/').next().unwrap_or(reference);
    if let Some(f) = fragments.get(basename) {
        return Some(f);
    }
    // Try $id-based lookup.
    fragments
        .values()
        .find(|f| f.id.as_deref() == Some(reference))
}

/// Detect cycles among fragments by following each fragment's $refs. Phase 1
/// is approximate — it follows refs by basename only; Phase 2 will do this
/// properly via jsonschema compilation.
fn detect_cycles(fragments: &HashMap<String, Fragment>) -> Vec<Vec<String>> {
    let mut cycles: Vec<Vec<String>> = Vec::new();
    for start in fragments.keys() {
        let mut stack: Vec<String> = vec![start.clone()];
        let mut seen: BTreeSet<String> = BTreeSet::new();
        seen.insert(start.clone());
        if dfs_for_cycle(start, fragments, &mut stack, &mut seen, &mut cycles) {
            break; // one is enough for the warning
        }
    }
    cycles
}

fn dfs_for_cycle(
    node: &str,
    fragments: &HashMap<String, Fragment>,
    stack: &mut Vec<String>,
    seen: &mut BTreeSet<String>,
    cycles: &mut Vec<Vec<String>>,
) -> bool {
    let Some(f) = fragments.get(node) else {
        return false;
    };
    for r in &f.refs {
        let basename = r.rsplit('/').next().unwrap_or(r).to_string();
        if !fragments.contains_key(&basename) {
            continue;
        }
        if let Some(idx) = stack.iter().position(|s| s == &basename) {
            let mut cycle: Vec<String> = stack[idx..].to_vec();
            cycle.push(basename);
            cycles.push(cycle);
            return true;
        }
        if seen.insert(basename.clone()) {
            stack.push(basename.clone());
            if dfs_for_cycle(&basename, fragments, stack, seen, cycles) {
                return true;
            }
            stack.pop();
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Phase-2 schema compilation: assemble the aggregator + inlined fragments
// into a `jsonschema::Validator` callers can reuse across module TOMLs.
// ---------------------------------------------------------------------------

/// Compile the aggregator schema into a reusable [`Validator`]. Returns
/// `Ok(None)` if the aggregator file is absent (skeleton projects). Errors
/// surface as findings on `report`; the caller decides whether to halt.
///
/// This runs the Phase-1 [`resolve`] walk first (which now treats cycles as
/// errors in Phase 2) and then performs the actual `jsonschema` build.
/// Any unresolved-`$ref` / duplicate-`$id` / cycle finding emitted by
/// `resolve` will be present in `report`; the caller should refuse to use
/// the returned validator if `report.has_errors()` is true.
#[instrument(level = "info", skip(layout, report), fields(aggregator = %layout.module_schema_path.display()))]
pub fn compile(layout: &LayoutContext, report: &mut Report) -> Result<Option<Arc<Validator>>> {
    let _summary = resolve(layout, report)?;

    if !layout.module_schema_path.is_file() {
        return Ok(None);
    }
    if report.findings().iter().any(|f| f.rule == "AGGREGATOR-003") {
        // Cycles are fatal at compile time — refuse to build a Validator.
        return Ok(None);
    }

    let docs = build_retriever_docs(layout)?;
    let aggregator_text = std::fs::read_to_string(&layout.module_schema_path)
        .with_context(|| format!("reading {}", layout.module_schema_path.display()))?;
    let aggregator_value: Value = serde_yaml::from_str(&aggregator_text).map_err(|e| {
        anyhow::Error::new(LocatedError::from_yaml_error(
            &layout.module_schema_path,
            &e,
        ))
    })?;

    let retriever = MapRetriever {
        docs: Arc::new(docs),
    };
    match jsonschema::options()
        .with_retriever(retriever)
        .build(&aggregator_value)
    {
        Ok(v) => Ok(Some(Arc::new(v))),
        Err(e) => {
            report.push(
                Finding::new(
                    "AGGREGATOR-005",
                    crate::domain::Severity::Error,
                    format!("AGGREGATOR-005 failed to compile aggregator schema: {e}"),
                )
                .with_file(layout.module_schema_path.display().to_string()),
            );
            Ok(None)
        }
    }
}

struct MapRetriever {
    docs: Arc<HashMap<String, Value>>,
}

impl Retrieve for MapRetriever {
    fn retrieve(
        &self,
        uri: &Uri<String>,
    ) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        let s = uri.as_str();
        if let Some(v) = self.docs.get(s) {
            return Ok(v.clone());
        }
        // Fall back to filename-only.
        if let Some(tail) = s.rsplit('/').next() {
            if let Some(v) = self.docs.get(tail) {
                return Ok(v.clone());
            }
        }
        // Try `inlined/blocks/<name>` and `inlined/bridges/<name>` style
        // paths used by the aggregator's `$ref` strings.
        for prefix in ["inlined/blocks/", "inlined/bridges/"] {
            if let Some(idx) = s.find(prefix) {
                let suffix = &s[idx..];
                if let Some(v) = self.docs.get(suffix) {
                    return Ok(v.clone());
                }
            }
        }
        Err(format!("schema not found in retriever: {s}").into())
    }
}

fn build_retriever_docs(layout: &LayoutContext) -> Result<HashMap<String, Value>> {
    let mut docs: HashMap<String, Value> = HashMap::new();
    for (dir, prefix) in [
        (&layout.inlined_blocks_dir, "inlined/blocks/"),
        (&layout.inlined_bridges_dir, "inlined/bridges/"),
    ] {
        if !dir.is_dir() {
            continue;
        }
        for entry in std::fs::read_dir(dir)
            .with_context(|| format!("reading {}", dir.display()))?
            .flatten()
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if !ext.eq_ignore_ascii_case("yaml") && !ext.eq_ignore_ascii_case("yml") {
                continue;
            }
            let fname = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let value: Value = serde_yaml::from_str(&text)
                .map_err(|e| anyhow::Error::new(LocatedError::from_yaml_error(&path, &e)))?;
            // Register under: $id, plain filename, and the relative path the
            // aggregator's $refs typically use.
            if let Some(id) = value.get("$id").and_then(Value::as_str) {
                docs.insert(id.to_string(), value.clone());
            }
            docs.insert(fname.clone(), value.clone());
            docs.insert(format!("{prefix}{fname}"), value);
        }
    }
    Ok(docs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn write_skeleton(root: &Path) {
        std::fs::create_dir_all(root.join("specs/specdev/schemas/inlined/blocks")).unwrap();
        std::fs::create_dir_all(root.join("specs/specdev/schemas/inlined/bridges")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("spec.toml"),
            r#"
spec_version = "1.0"

[project]
name = "agg"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
        )
        .unwrap();
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "specdev-aggregator-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn empty_skeleton_returns_zero_counts() {
        let root = tmp("empty");
        write_skeleton(&root);
        let layout = LayoutContext::discover(&root, None).unwrap();
        let mut report = Report::default();
        let summary = resolve(&layout, &mut report).unwrap();
        assert_eq!(summary.fragments_loaded, 0);
        assert_eq!(summary.refs_total, 0);
        assert!(!report.has_errors());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unresolved_ref_reports_error() {
        let root = tmp("unresolved");
        write_skeleton(&root);
        std::fs::write(
            root.join("specs/specdev/schemas/module.schema.yaml"),
            r#"
$schema: "https://json-schema.org/draft/2020-12/schema"
type: object
allOf:
  - $ref: "inlined/blocks/missing.schema.yaml"
"#,
        )
        .unwrap();
        let layout = LayoutContext::discover(&root, None).unwrap();
        let mut report = Report::default();
        let summary = resolve(&layout, &mut report).unwrap();
        assert_eq!(summary.refs_unresolved, 1);
        assert!(report.has_errors());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn duplicate_id_reports_error() {
        let root = tmp("dup-id");
        write_skeleton(&root);
        std::fs::write(
            root.join("specs/specdev/schemas/inlined/blocks/a.schema.yaml"),
            r#"
$id: "https://specdev.dev/blocks/dup.v1"
type: object
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/schemas/inlined/bridges/b.schema.yaml"),
            r#"
$id: "https://specdev.dev/blocks/dup.v1"
type: object
"#,
        )
        .unwrap();
        let layout = LayoutContext::discover(&root, None).unwrap();
        let mut report = Report::default();
        let _summary = resolve(&layout, &mut report).unwrap();
        assert!(report.has_errors());
        assert!(report.findings().iter().any(|f| f.rule == "AGGREGATOR-002"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolved_ref_passes() {
        let root = tmp("resolved");
        write_skeleton(&root);
        std::fs::write(
            root.join("specs/specdev/schemas/inlined/blocks/dummy.schema.yaml"),
            r#"
$id: "https://specdev.dev/blocks/dummy.v1"
type: object
"#,
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/schemas/module.schema.yaml"),
            r#"
$schema: "https://json-schema.org/draft/2020-12/schema"
type: object
allOf:
  - $ref: "inlined/blocks/dummy.schema.yaml"
"#,
        )
        .unwrap();
        let layout = LayoutContext::discover(&root, None).unwrap();
        let mut report = Report::default();
        let summary = resolve(&layout, &mut report).unwrap();
        assert_eq!(summary.fragments_loaded, 1);
        assert_eq!(summary.refs_total, 1);
        assert_eq!(summary.refs_resolved, 1);
        assert_eq!(summary.refs_unresolved, 0);
        assert!(!report.has_errors());
        let _ = std::fs::remove_dir_all(&root);
    }
}
