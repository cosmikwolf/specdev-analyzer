//! Layout-level reduction loader and path-qualified resolver.
//!
//! Two layers:
//!
//! 1. [`load_all`] walks `reductions/project.yaml`,
//!    `reductions/blocks/*.yaml`, `reductions/bridges/*.yaml`, and
//!    `reductions/bridges/local/*.yaml`, parses each to a
//!    `serde_yaml::Value`, and computes the namespace from filesystem
//!    location. Pure I/O + parsing — no `extends:` resolution, no kind
//!    path-qualification.
//! 2. [`resolve`] takes the [`ReductionInventory`] and produces
//!    an [`crate::resolved::EffectiveKindSet`]: rewrites unqualified
//!    declarations to qualified form, validates the path grammar,
//!    resolves `extends` chains via fixed-point iteration, and detects
//!    canonical-vs-local bridge shadowing. Findings are returned
//!    alongside the kind set; the caller pushes them into a
//!    [`crate::report::Report`].
//!
//! [`crate::reduction`] is the executor that consumes
//! [`crate::resolved::EffectiveKindSet`] to produce the project graph.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use tracing::{event, instrument, Level};

use crate::canonical::{canonical_relation_kinds, canonical_root_kinds};
use crate::diagnostics::LocatedError;
use crate::domain::Severity;
use crate::kinds::{self, KindPath};
use crate::layout::LayoutContext;
use crate::report::Finding;
use crate::resolved::{
    EffectiveKindSet, ResolvedEntityDecl, ResolvedExtends, ResolvedRelationDecl,
};

#[derive(Debug, Clone)]
pub struct ReductionInventory {
    pub project: Option<RawReduction>,
    pub blocks: Vec<RawReduction>,
    pub bridges_canonical: Vec<RawReduction>,
    pub bridges_local: Vec<RawReduction>,
}

impl ReductionInventory {
    pub fn total(&self) -> usize {
        let mut n = self.project.as_ref().map_or(0, |_| 1);
        n += self.blocks.len();
        n += self.bridges_canonical.len();
        n += self.bridges_local.len();
        n
    }
}

#[derive(Debug, Clone)]
pub struct RawReduction {
    pub source_path: PathBuf,
    pub namespace: ReductionNamespace,
    pub raw: serde_yaml::Value,
    pub kind_declarations: Vec<RawKindDecl>,
    /// Original YAML text, retained for source-line lookup during
    /// resolution. Re-reading from disk would work but this avoids a second
    /// I/O per file when resolve() is called.
    pub source_text: String,
}

#[derive(Debug, Clone)]
pub enum ReductionNamespace {
    Project,
    Block(String),
    BridgeCanonical { left: String, right: String },
    BridgeLocal { left: String, right: String },
}

impl ReductionNamespace {
    pub fn label(&self) -> String {
        match self {
            ReductionNamespace::Project => "project".to_string(),
            ReductionNamespace::Block(b) => b.clone(),
            ReductionNamespace::BridgeCanonical { left, right } => format!("{left}+{right}"),
            ReductionNamespace::BridgeLocal { left, right } => format!("local:{left}+{right}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RawKindDecl {
    pub kind_name: String,
    pub extends: Option<String>,
    pub source_line: u32,
}

#[instrument(level = "info", skip(layout), fields(reductions_dir = %layout.reductions_dir.display()))]
pub fn load_all(layout: &LayoutContext) -> Result<ReductionInventory> {
    let project = if layout.project_reduction_path.is_file() {
        Some(load_one(
            &layout.project_reduction_path,
            ReductionNamespace::Project,
        )?)
    } else {
        None
    };

    let blocks = list_yaml(&layout.reductions_blocks_dir)
        .into_iter()
        .map(|p| {
            let stem = file_stem(&p);
            let ns = ReductionNamespace::Block(stem);
            load_one(&p, ns)
        })
        .collect::<Result<Vec<_>>>()?;

    let bridges_canonical = list_yaml(&layout.reductions_bridges_dir)
        .into_iter()
        // Skip the "local" subdirectory.
        .filter(|p| {
            p.parent()
                .map(|d| d == layout.reductions_bridges_dir.as_path())
                .unwrap_or(false)
        })
        .map(|p| {
            let stem = file_stem(&p);
            let ns = parse_bridge_namespace(&stem, false).unwrap_or_else(|| {
                ReductionNamespace::BridgeCanonical {
                    left: stem.clone(),
                    right: stem,
                }
            });
            load_one(&p, ns)
        })
        .collect::<Result<Vec<_>>>()?;

    let bridges_local = list_yaml(&layout.reductions_bridges_local_dir)
        .into_iter()
        .map(|p| {
            let stem = file_stem(&p);
            let ns = parse_bridge_namespace(&stem, true).unwrap_or_else(|| {
                ReductionNamespace::BridgeLocal {
                    left: stem.clone(),
                    right: stem,
                }
            });
            load_one(&p, ns)
        })
        .collect::<Result<Vec<_>>>()?;

    let inv = ReductionInventory {
        project,
        blocks,
        bridges_canonical,
        bridges_local,
    };

    for r in inv.iter_all() {
        event!(
            Level::DEBUG,
            namespace = %r.namespace.label(),
            kind_decls = r.kind_declarations.len(),
            source = %r.source_path.display(),
            "reduction_file_loaded",
        );
    }

    Ok(inv)
}

impl ReductionInventory {
    pub fn iter_all(&self) -> impl Iterator<Item = &RawReduction> {
        self.project
            .iter()
            .chain(self.blocks.iter())
            .chain(self.bridges_canonical.iter())
            .chain(self.bridges_local.iter())
    }
}

fn load_one(path: &Path, namespace: ReductionNamespace) -> Result<RawReduction> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let raw: serde_yaml::Value = match serde_yaml::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return Err(anyhow::Error::new(LocatedError::from_yaml_error(path, &e)));
        }
    };
    let kind_declarations = collect_kind_declarations(&raw, &text);
    Ok(RawReduction {
        source_path: path.to_path_buf(),
        namespace,
        raw,
        kind_declarations,
        source_text: text,
    })
}

/// Shallow walk the parsed YAML for `kind:` and `extends:` strings inside
/// the conventional `entities:` and `relations:` lists.
fn collect_kind_declarations(raw: &serde_yaml::Value, source_text: &str) -> Vec<RawKindDecl> {
    let mut out = Vec::new();
    // Translate to JSON for easy walking — both formats share a tree
    // shape. We don't need YAML-specific information here.
    let Ok(json): Result<JsonValue, _> = serde_yaml::from_value(raw.clone()) else {
        return out;
    };
    for key in ["entities", "relations"] {
        if let Some(arr) = json.get(key).and_then(JsonValue::as_array) {
            for item in arr {
                let Some(obj) = item.as_object() else {
                    continue;
                };
                let Some(kind_name) = obj.get("kind").and_then(JsonValue::as_str) else {
                    continue;
                };
                let extends = obj
                    .get("extends")
                    .and_then(JsonValue::as_str)
                    .map(|s| s.to_string());
                let source_line = find_line_of(source_text, kind_name);
                out.push(RawKindDecl {
                    kind_name: kind_name.to_string(),
                    extends,
                    source_line,
                });
            }
        }
    }
    out
}

fn find_line_of(source: &str, needle: &str) -> u32 {
    let needle_quoted = format!("\"{needle}\"");
    for (i, line) in source.lines().enumerate() {
        if line.contains(needle) || line.contains(&needle_quoted) {
            return (i + 1) as u32;
        }
    }
    0
}

fn list_yaml(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string()
}

fn parse_bridge_namespace(stem: &str, is_local: bool) -> Option<ReductionNamespace> {
    let (left, right) = stem.split_once('+')?;
    if left.is_empty() || right.is_empty() {
        return None;
    }
    if is_local {
        Some(ReductionNamespace::BridgeLocal {
            left: left.to_string(),
            right: right.to_string(),
        })
    } else {
        Some(ReductionNamespace::BridgeCanonical {
            left: left.to_string(),
            right: right.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Resolver: open-vocabulary kind resolution.
// ---------------------------------------------------------------------------

/// Walk every loaded reduction, rewrite unqualified kind names to qualified
/// form by file location, validate the path grammar, resolve `extends`
/// chains to canonical anchors, and produce the [`EffectiveKindSet`].
///
/// Findings emitted (rule codes):
///
/// * `METASCHEMA-001` — non-canonical entity/relation missing `extends:`.
/// * `METASCHEMA-002` — `extends:` target does not name a canonical anchor
///   nor a previously-resolved qualified kind. Catches both unresolved
///   references and `extends` cycles.
/// * `METASCHEMA-003` — bridge filename is not lex-ordered (e.g.
///   `lvgl+embassy.yaml` instead of `embassy+lvgl.yaml`).
/// * `METASCHEMA-004` — same `<a>+<b>` pair is declared in both
///   `bridges/` and `bridges/local/`. Warning; canonical wins.
/// * `METASCHEMA-005` — kind path violates the grammar
///   (`Type` / `<block>.<Kind>` / `<a>+<b>.<relation>`).
///
/// Lex-order is also enforced inside [`crate::kinds::parse`] for path
/// strings inside reduction bodies; the filesystem-name check here is the
/// distinct, complementary check — lex-order is enforced in these two
/// places intentionally.
pub fn resolve(inv: &ReductionInventory) -> (EffectiveKindSet, Vec<Finding>) {
    let mut findings: Vec<Finding> = Vec::new();

    bridge_filesystem_checks(inv, &mut findings);

    // Canonical-vs-local shadow: shadowed local bridges are warned about
    // (METASCHEMA-004) but excluded from declaration walking — canonical
    // wins.
    let canonical_pairs: BTreeSet<String> = inv
        .bridges_canonical
        .iter()
        .filter_map(|r| match &r.namespace {
            ReductionNamespace::BridgeCanonical { left, right } => Some(format!("{left}+{right}")),
            _ => None,
        })
        .collect();

    let walk_order: Vec<&RawReduction> = inv
        .project
        .iter()
        .chain(inv.blocks.iter())
        .chain(inv.bridges_canonical.iter())
        .chain(inv.bridges_local.iter().filter(|r| match &r.namespace {
            ReductionNamespace::BridgeLocal { left, right } => {
                !canonical_pairs.contains(&format!("{left}+{right}"))
            }
            _ => true,
        }))
        .collect();

    let mut pending_entities: Vec<PendingEntity> = Vec::new();
    let mut pending_relations: Vec<PendingRelation> = Vec::new();
    for r in walk_order {
        walk_file(
            r,
            &mut pending_entities,
            &mut pending_relations,
            &mut findings,
        );
    }

    let (entities, relations) =
        resolve_extends_chains(pending_entities, pending_relations, &mut findings);

    let kindset = EffectiveKindSet {
        canonical_roots: canonical_root_kinds().clone(),
        canonical_relations: canonical_relation_kinds().clone(),
        entities,
        relations,
    };
    (kindset, findings)
}

fn bridge_filesystem_checks(inv: &ReductionInventory, findings: &mut Vec<Finding>) {
    // METASCHEMA-003 — lex-order on bridge filesystem names. Distinct from
    // the path-string lex-order check inside `kinds::parse`; both point at
    // the same human error class but catch it at different lifecycle stages.
    for r in inv.bridges_canonical.iter().chain(inv.bridges_local.iter()) {
        let (left, right) = match &r.namespace {
            ReductionNamespace::BridgeCanonical { left, right }
            | ReductionNamespace::BridgeLocal { left, right } => (left, right),
            _ => continue,
        };
        if left >= right {
            findings.push(
                Finding::new(
                    "METASCHEMA-003",
                    Severity::Error,
                    format!(
                        "bridge filename `{}` is not lex-ordered: rename to `{}+{}.yaml` (left < right)",
                        r.source_path
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or("?"),
                        right,
                        left
                    ),
                )
                .with_file(r.source_path.display().to_string()),
            );
        }
    }

    // METASCHEMA-004 — canonical/local shadow. Warning;
    // canonical takes precedence.
    let canonical_files: BTreeMap<String, PathBuf> = inv
        .bridges_canonical
        .iter()
        .filter_map(|r| match &r.namespace {
            ReductionNamespace::BridgeCanonical { left, right } => {
                Some((format!("{left}+{right}"), r.source_path.clone()))
            }
            _ => None,
        })
        .collect();
    for r in &inv.bridges_local {
        let key = match &r.namespace {
            ReductionNamespace::BridgeLocal { left, right } => format!("{left}+{right}"),
            _ => continue,
        };
        if let Some(canonical_path) = canonical_files.get(&key) {
            findings.push(
                Finding::new(
                    "METASCHEMA-004",
                    Severity::Warning,
                    format!(
                        "local bridge `{}` is shadowed by canonical bridge `{}`; canonical wins. \
                         Remove or rename one to silence this warning.",
                        r.source_path.display(),
                        canonical_path.display()
                    ),
                )
                .with_file(r.source_path.display().to_string()),
            );
        }
    }
}

#[derive(Debug, Clone)]
struct PendingEntity {
    qualified: String,
    extends_string: String,
    decl: ResolvedEntityDecl,
}

#[derive(Debug, Clone)]
struct PendingRelation {
    qualified: String,
    extends_string: String,
    decl: ResolvedRelationDecl,
}

fn walk_file(
    r: &RawReduction,
    pending_entities: &mut Vec<PendingEntity>,
    pending_relations: &mut Vec<PendingRelation>,
    findings: &mut Vec<Finding>,
) {
    // Convert YAML → JSON for easy walking; both share tree shape.
    let json: JsonValue = match serde_yaml::from_value(r.raw.clone()) {
        Ok(v) => v,
        Err(_) => return,
    };

    if let Some(arr) = json.get("entities").and_then(JsonValue::as_array) {
        for (idx, item) in arr.iter().enumerate() {
            let Some(obj) = item.as_object() else {
                continue;
            };
            walk_entity(r, idx, obj, pending_entities, findings);
        }
    }

    if let Some(arr) = json.get("relations").and_then(JsonValue::as_array) {
        for (idx, item) in arr.iter().enumerate() {
            let Some(obj) = item.as_object() else {
                continue;
            };
            walk_relation(r, idx, obj, pending_relations, findings);
        }
    }
}

fn walk_entity(
    r: &RawReduction,
    _idx: usize,
    obj: &serde_json::Map<String, JsonValue>,
    pending: &mut Vec<PendingEntity>,
    findings: &mut Vec<Finding>,
) {
    let Some(kind_str) = obj.get("kind").and_then(JsonValue::as_str) else {
        return;
    };
    let source_line = find_line_of(&r.source_text, kind_str);

    // Bridges declare relations only.
    if matches!(
        r.namespace,
        ReductionNamespace::BridgeCanonical { .. } | ReductionNamespace::BridgeLocal { .. }
    ) {
        findings.push(
            Finding::new(
                "METASCHEMA-005",
                Severity::Error,
                format!(
                    "bridge reductions declare relations only; entity `{}` is not allowed in {}",
                    kind_str,
                    r.source_path.display()
                ),
            )
            .with_file(r.source_path.display().to_string())
            .with_line(source_line),
        );
        return;
    }

    let qualified_path = match qualify_declared_kind(kind_str, &r.namespace, true) {
        Ok(p) => p,
        Err(msg) => {
            findings.push(
                Finding::new("METASCHEMA-005", Severity::Error, msg)
                    .with_file(r.source_path.display().to_string())
                    .with_line(source_line),
            );
            return;
        }
    };

    let extends_str = match obj.get("extends").and_then(JsonValue::as_str) {
        Some(s) => s.to_string(),
        None => {
            findings.push(
                Finding::new(
                    "METASCHEMA-001",
                    Severity::Error,
                    format!("entity `{}` is missing required `extends:`", qualified_path),
                )
                .with_file(r.source_path.display().to_string())
                .with_line(source_line),
            );
            return;
        }
    };

    let raw_yaml: serde_yaml::Value = serde_json::from_value(JsonValue::Object(obj.clone()))
        .ok()
        .and_then(|j: JsonValue| serde_yaml::to_value(j).ok())
        .unwrap_or(serde_yaml::Value::Null);

    let decl = ResolvedEntityDecl {
        path: qualified_path.clone(),
        extends: ResolvedExtends::Canonical(String::new()), // placeholder, filled in resolve_extends_chains
        source_path: r.source_path.clone(),
        source_line,
        raw: raw_yaml,
    };
    pending.push(PendingEntity {
        qualified: qualified_path.to_string(),
        extends_string: extends_str,
        decl,
    });
}

fn walk_relation(
    r: &RawReduction,
    _idx: usize,
    obj: &serde_json::Map<String, JsonValue>,
    pending: &mut Vec<PendingRelation>,
    findings: &mut Vec<Finding>,
) {
    let Some(kind_str) = obj.get("kind").and_then(JsonValue::as_str) else {
        return;
    };
    let source_line = find_line_of(&r.source_text, kind_str);

    let qualified_path = match qualify_declared_kind(kind_str, &r.namespace, false) {
        Ok(p) => p,
        Err(msg) => {
            findings.push(
                Finding::new("METASCHEMA-005", Severity::Error, msg)
                    .with_file(r.source_path.display().to_string())
                    .with_line(source_line),
            );
            return;
        }
    };

    let extends_str = match obj.get("extends").and_then(JsonValue::as_str) {
        Some(s) => s.to_string(),
        None => {
            findings.push(
                Finding::new(
                    "METASCHEMA-001",
                    Severity::Error,
                    format!(
                        "relation `{}` is missing required `extends:`",
                        qualified_path
                    ),
                )
                .with_file(r.source_path.display().to_string())
                .with_line(source_line),
            );
            return;
        }
    };

    let from_kind = match obj
        .get("from_kind")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "missing `from_kind`".to_string())
        .and_then(|s| qualify_endpoint_kind(s, &r.namespace))
    {
        Ok(p) => p,
        Err(msg) => {
            findings.push(
                Finding::new(
                    "METASCHEMA-005",
                    Severity::Error,
                    format!("relation `{qualified_path}`: {msg}"),
                )
                .with_file(r.source_path.display().to_string())
                .with_line(source_line),
            );
            return;
        }
    };

    let to_kind = match obj
        .get("to_kind")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| "missing `to_kind`".to_string())
        .and_then(|s| qualify_endpoint_kind(s, &r.namespace))
    {
        Ok(p) => p,
        Err(msg) => {
            findings.push(
                Finding::new(
                    "METASCHEMA-005",
                    Severity::Error,
                    format!("relation `{qualified_path}`: {msg}"),
                )
                .with_file(r.source_path.display().to_string())
                .with_line(source_line),
            );
            return;
        }
    };

    let raw_yaml: serde_yaml::Value = serde_json::from_value(JsonValue::Object(obj.clone()))
        .ok()
        .and_then(|j: JsonValue| serde_yaml::to_value(j).ok())
        .unwrap_or(serde_yaml::Value::Null);

    let decl = ResolvedRelationDecl {
        path: qualified_path.clone(),
        extends: ResolvedExtends::Canonical(String::new()), // placeholder
        from_kind,
        to_kind,
        source_path: r.source_path.clone(),
        source_line,
        raw: raw_yaml,
    };
    pending.push(PendingRelation {
        qualified: qualified_path.to_string(),
        extends_string: extends_str,
        decl,
    });
}

/// Rewrite a `kind:` value to its qualified form based on the file's
/// namespace. `is_entity` selects PascalCase vs snake_case grammar
/// expectations; bridges only ever call this with `is_entity = false`
/// (that case is filtered before the call).
fn qualify_declared_kind(
    kind_str: &str,
    namespace: &ReductionNamespace,
    is_entity: bool,
) -> Result<KindPath, String> {
    // First, parse the literal to see what shape the user wrote.
    let parsed = kinds::parse(kind_str).map_err(|e| e.to_string())?;
    match (&parsed, namespace) {
        // Unqualified: rewrite to the file's namespace.
        (KindPath::Unqualified(name), ReductionNamespace::Project) => {
            // Project namespace stays unqualified.
            if is_entity && !name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                return Err(format!(
                    "entity kinds must be PascalCase; `{name}` is not"
                ));
            }
            if !is_entity && name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                return Err(format!(
                    "relation kinds must be snake_case; `{name}` is not"
                ));
            }
            Ok(parsed)
        }
        (KindPath::Unqualified(name), ReductionNamespace::Block(block)) => Ok(KindPath::Block {
            block: block.clone(),
            name: name.clone(),
        }),
        (KindPath::Unqualified(name), ReductionNamespace::BridgeCanonical { left, right })
        | (KindPath::Unqualified(name), ReductionNamespace::BridgeLocal { left, right }) => {
            if is_entity {
                // Filtered earlier; defensive.
                Err("bridge files cannot declare entities".into())
            } else {
                Ok(KindPath::Bridge {
                    left: left.clone(),
                    right: right.clone(),
                    relation: name.clone(),
                })
            }
        }

        // Already-qualified: must match the file's namespace.
        (KindPath::Block { block, .. }, ReductionNamespace::Block(file_block)) => {
            if block != file_block {
                Err(format!(
                    "kind `{kind_str}` declares namespace `{block}` but file is in namespace `{file_block}`",
                ))
            } else {
                Ok(parsed)
            }
        }
        (KindPath::Bridge { left, right, .. }, ReductionNamespace::BridgeCanonical { left: fl, right: fr })
        | (KindPath::Bridge { left, right, .. }, ReductionNamespace::BridgeLocal { left: fl, right: fr }) => {
            if left != fl || right != fr {
                Err(format!(
                    "kind `{kind_str}` declares namespace `{left}+{right}` but file is in namespace `{fl}+{fr}`",
                ))
            } else {
                Ok(parsed)
            }
        }

        // Cross-namespace declarations are not allowed in `kind:`.
        (KindPath::Block { block, .. }, ReductionNamespace::Project) => Err(format!(
            "project reduction cannot declare block-qualified kind `{kind_str}` (block `{block}` belongs in `blocks/{block}.yaml`)",
        )),
        (KindPath::Bridge { .. }, ReductionNamespace::Project) => Err(format!(
            "project reduction cannot declare bridge-qualified kind `{kind_str}`",
        )),
        (KindPath::Bridge { .. }, ReductionNamespace::Block(b)) => Err(format!(
            "block `{b}` cannot declare bridge-qualified kind `{kind_str}`",
        )),
        (KindPath::Block { .. }, ReductionNamespace::BridgeCanonical { .. })
        | (KindPath::Block { .. }, ReductionNamespace::BridgeLocal { .. }) => Err(format!(
            "bridge cannot declare block-qualified kind `{kind_str}`",
        )),
    }
}

/// Rewrite a `from_kind:`/`to_kind:` value based on the file's namespace.
/// Bare names resolve under the file's namespace; full paths pass through
/// after grammar validation. Bridges always use full paths (entities are
/// foreign to the bridge by construction); a bare name in a bridge resolves
/// to one of the bridge's owning blocks — that ambiguity is rejected.
fn qualify_endpoint_kind(s: &str, namespace: &ReductionNamespace) -> Result<KindPath, String> {
    let parsed = kinds::parse(s).map_err(|e| e.to_string())?;
    match (&parsed, namespace) {
        (KindPath::Unqualified(_), ReductionNamespace::Project) => Ok(parsed),
        (KindPath::Unqualified(name), ReductionNamespace::Block(block)) => Ok(KindPath::Block {
            block: block.clone(),
            name: name.clone(),
        }),
        (KindPath::Unqualified(_), ReductionNamespace::BridgeCanonical { .. })
        | (KindPath::Unqualified(_), ReductionNamespace::BridgeLocal { .. }) => Err(format!(
            "bridge endpoints must use full paths; bare `{s}` is ambiguous between the two owners",
        )),
        // Block-qualified or already-bridge-qualified pass through.
        _ => Ok(parsed),
    }
}

/// Fixed-point resolution of `extends` chains. After each pass, every
/// pending decl whose extends string matches a canonical anchor or an
/// already-resolved qualified entry is moved into the resolved map.
/// When no progress is made, remaining pending entries are reported as
/// `METASCHEMA-002` (unresolved).
fn resolve_extends_chains(
    pending_entities: Vec<PendingEntity>,
    pending_relations: Vec<PendingRelation>,
    findings: &mut Vec<Finding>,
) -> (
    BTreeMap<String, ResolvedEntityDecl>,
    BTreeMap<String, ResolvedRelationDecl>,
) {
    let mut entities: BTreeMap<String, ResolvedEntityDecl> = BTreeMap::new();
    let mut relations: BTreeMap<String, ResolvedRelationDecl> = BTreeMap::new();

    let mut pending_e = pending_entities;
    loop {
        let before = pending_e.len();
        pending_e.retain(|p| {
            // Try canonical-root first.
            if canonical_root_kinds().contains(&p.extends_string) {
                let mut decl = p.decl.clone();
                decl.extends = ResolvedExtends::Canonical(p.extends_string.clone());
                entities.insert(p.qualified.clone(), decl);
                return false;
            }
            // Try previously-resolved qualified entity.
            if let Some(_target) = entities.get(&p.extends_string) {
                let target_path = match kinds::parse(&p.extends_string) {
                    Ok(kp) => kp,
                    Err(_) => return true, // shouldn't happen at this point
                };
                let mut decl = p.decl.clone();
                decl.extends = ResolvedExtends::Qualified(target_path);
                entities.insert(p.qualified.clone(), decl);
                return false;
            }
            true
        });
        if pending_e.len() == before {
            break;
        }
    }

    for p in pending_e {
        findings.push(
            Finding::new(
                "METASCHEMA-002",
                Severity::Error,
                format!(
                    "entity `{}`: `extends: {}` does not name a canonical root \
                     (one of: {}) nor a previously-resolved qualified entity. \
                     Either declare a known root or fix the typo (cycles in `extends` chains \
                     also surface here).",
                    p.qualified,
                    p.extends_string,
                    canonical_list(canonical_root_kinds()),
                ),
            )
            .with_file(p.decl.source_path.display().to_string())
            .with_line(p.decl.source_line),
        );
    }

    let mut pending_r = pending_relations;
    loop {
        let before = pending_r.len();
        pending_r.retain(|p| {
            if canonical_relation_kinds().contains(&p.extends_string) {
                let mut decl = p.decl.clone();
                decl.extends = ResolvedExtends::Canonical(p.extends_string.clone());
                relations.insert(p.qualified.clone(), decl);
                return false;
            }
            if relations.contains_key(&p.extends_string) {
                let target_path = match kinds::parse(&p.extends_string) {
                    Ok(kp) => kp,
                    Err(_) => return true,
                };
                let mut decl = p.decl.clone();
                decl.extends = ResolvedExtends::Qualified(target_path);
                relations.insert(p.qualified.clone(), decl);
                return false;
            }
            true
        });
        if pending_r.len() == before {
            break;
        }
    }

    for p in pending_r {
        findings.push(
            Finding::new(
                "METASCHEMA-002",
                Severity::Error,
                format!(
                    "relation `{}`: `extends: {}` does not name a canonical relation \
                     (one of: {}) nor a previously-resolved qualified relation. \
                     Either declare a known relation or fix the typo (cycles in `extends` chains \
                     also surface here).",
                    p.qualified,
                    p.extends_string,
                    canonical_list(canonical_relation_kinds()),
                ),
            )
            .with_file(p.decl.source_path.display().to_string())
            .with_line(p.decl.source_line),
        );
    }

    (entities, relations)
}

fn canonical_list(set: &BTreeSet<String>) -> String {
    let v: Vec<String> = set.iter().cloned().collect();
    v.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "specdev-reductions-{tag}-{}-{}",
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

    fn write_skeleton(root: &Path) {
        std::fs::create_dir_all(root.join("specs/specdev/reductions/blocks")).unwrap();
        std::fs::create_dir_all(root.join("specs/specdev/reductions/bridges/local")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("spec.toml"),
            r#"
spec_version = "1.0"

[project]
name = "red"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
        )
        .unwrap();
    }

    #[test]
    fn empty_returns_empty_inventory() {
        let root = tmp("empty");
        write_skeleton(&root);
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        assert!(inv.project.is_none());
        assert!(inv.blocks.is_empty());
        assert!(inv.bridges_canonical.is_empty());
        assert!(inv.bridges_local.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn classifies_namespaces_from_paths() {
        let root = tmp("ns");
        write_skeleton(&root);
        std::fs::write(
            root.join("specs/specdev/reductions/project.yaml"),
            "entities: []\nrelations: []\n",
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/reductions/blocks/embassy.yaml"),
            "entities: [{kind: Task, extends: Module}]\nrelations: []\n",
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/reductions/bridges/embassy+lvgl.yaml"),
            "entities: []\nrelations: [{kind: renders, extends: depends_on}]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("specs/specdev/reductions/bridges/local/foo+bar.yaml"),
            "entities: []\nrelations: []\n",
        )
        .unwrap();

        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        assert!(inv.project.is_some());
        assert_eq!(inv.blocks.len(), 1);
        match &inv.blocks[0].namespace {
            ReductionNamespace::Block(b) => assert_eq!(b, "embassy"),
            other => panic!("expected Block(embassy), got {:?}", other),
        }
        assert_eq!(inv.bridges_canonical.len(), 1);
        match &inv.bridges_canonical[0].namespace {
            ReductionNamespace::BridgeCanonical { left, right } => {
                assert_eq!(left, "embassy");
                assert_eq!(right, "lvgl");
            }
            other => panic!("expected BridgeCanonical(embassy+lvgl), got {:?}", other),
        }
        assert_eq!(inv.bridges_local.len(), 1);
        match &inv.bridges_local[0].namespace {
            ReductionNamespace::BridgeLocal { left, right } => {
                assert_eq!(left, "foo");
                assert_eq!(right, "bar");
            }
            other => panic!("expected BridgeLocal(foo+bar), got {:?}", other),
        }
        // Block with one kind declaration.
        assert_eq!(inv.blocks[0].kind_declarations.len(), 1);
        assert_eq!(inv.blocks[0].kind_declarations[0].kind_name, "Task");
        assert_eq!(
            inv.blocks[0].kind_declarations[0].extends.as_deref(),
            Some("Module")
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------
    // Resolver tests
    // -----------------------------------------------------------------

    fn write_project_yaml(root: &Path, body: &str) {
        std::fs::create_dir_all(root.join("specs/specdev/reductions")).unwrap();
        std::fs::write(root.join("specs/specdev/reductions/project.yaml"), body).unwrap();
    }

    fn write_block(root: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(root.join("specs/specdev/reductions/blocks")).unwrap();
        std::fs::write(
            root.join(format!("specs/specdev/reductions/blocks/{name}.yaml")),
            body,
        )
        .unwrap();
    }

    fn write_bridge(root: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(root.join("specs/specdev/reductions/bridges")).unwrap();
        std::fs::write(
            root.join(format!("specs/specdev/reductions/bridges/{name}.yaml")),
            body,
        )
        .unwrap();
    }

    fn write_local_bridge(root: &Path, name: &str, body: &str) {
        std::fs::create_dir_all(root.join("specs/specdev/reductions/bridges/local")).unwrap();
        std::fs::write(
            root.join(format!(
                "specs/specdev/reductions/bridges/local/{name}.yaml"
            )),
            body,
        )
        .unwrap();
    }

    #[test]
    fn resolve_block_entity_qualifies_and_resolves_to_canonical() {
        let root = tmp("resolve-block-ok");
        write_skeleton(&root);
        write_block(
            &root,
            "embassy",
            r#"metaschema_version: "v1"
entities:
  - kind: Task
    extends: structural
    from: "specs/modules/*.toml"
    name: "${.name}"
relations: []
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (kindset, findings) = resolve(&inv);
        assert!(!findings.iter().any(|f| f.severity == Severity::Error));
        let task = kindset.entity("embassy.Task").expect("embassy.Task entity");
        assert_eq!(
            task.extends,
            ResolvedExtends::Canonical("structural".into())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_chain_through_profile_unqualified_kind() {
        let root = tmp("resolve-chain");
        write_skeleton(&root);
        write_project_yaml(
            &root,
            r#"metaschema_version: "v1"
entities:
  - kind: Module
    extends: structural
    from: "specs/modules/*.toml"
    name: "${.name}"
relations: []
"#,
        );
        write_block(
            &root,
            "embassy",
            r#"metaschema_version: "v1"
entities:
  - kind: Task
    extends: Module
    from: "specs/modules/*.toml"
    name: "${.name}"
relations: []
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (kindset, findings) = resolve(&inv);
        assert!(
            !findings.iter().any(|f| f.severity == Severity::Error),
            "unexpected findings: {:?}",
            findings
        );
        let task = kindset.entity("embassy.Task").unwrap();
        match &task.extends {
            ResolvedExtends::Qualified(p) => assert_eq!(p.to_string(), "Module"),
            other => panic!("expected Qualified(Module), got {other:?}"),
        }
        // Chain walks through: embassy.Task → Module → structural.
        assert_eq!(
            kindset.entity_canonical_root("embassy.Task").as_deref(),
            Some("structural")
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_emits_metaschema_001_on_missing_extends() {
        let root = tmp("missing-extends");
        write_skeleton(&root);
        write_block(
            &root,
            "embassy",
            r#"metaschema_version: "v1"
entities:
  - kind: Task
    from: "specs/modules/*.toml"
    name: "${.name}"
relations: []
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (_kindset, findings) = resolve(&inv);
        assert!(findings.iter().any(|f| f.rule == "METASCHEMA-001"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_emits_metaschema_002_on_unresolved_extends() {
        let root = tmp("unresolved");
        write_skeleton(&root);
        write_block(
            &root,
            "embassy",
            r#"metaschema_version: "v1"
entities:
  - kind: Task
    extends: NotARealKind
    from: "specs/modules/*.toml"
    name: "${.name}"
relations: []
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (_, findings) = resolve(&inv);
        assert!(findings.iter().any(|f| f.rule == "METASCHEMA-002"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_emits_metaschema_002_on_extends_cycle() {
        let root = tmp("cycle");
        write_skeleton(&root);
        write_project_yaml(
            &root,
            r#"metaschema_version: "v1"
entities:
  - kind: A
    extends: B
    from: "specs/modules/*.toml"
    name: "${.name}"
  - kind: B
    extends: A
    from: "specs/modules/*.toml"
    name: "${.name}"
relations: []
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (_, findings) = resolve(&inv);
        // Both entities should fail to resolve and surface as METASCHEMA-002.
        let count = findings
            .iter()
            .filter(|f| f.rule == "METASCHEMA-002")
            .count();
        assert_eq!(count, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_emits_metaschema_003_on_lex_order_violation() {
        let root = tmp("lex-order");
        write_skeleton(&root);
        // Filename `lvgl+embassy.yaml` violates lex ordering.
        write_bridge(
            &root,
            "lvgl+embassy",
            r#"metaschema_version: "v1"
entities: []
relations: []
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (_, findings) = resolve(&inv);
        assert!(findings.iter().any(|f| f.rule == "METASCHEMA-003"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_emits_metaschema_004_on_canonical_local_shadow() {
        let root = tmp("shadow");
        write_skeleton(&root);
        write_bridge(
            &root,
            "embassy+lvgl",
            r#"metaschema_version: "v1"
entities: []
relations:
  - kind: renders
    extends: depends_on
    from_kind: embassy.Task
    to_kind: lvgl.Widget
    from_spec: "specs/modules/*.toml"
    endpoints: { from: "${.from}", to: "${.to}" }
"#,
        );
        write_local_bridge(
            &root,
            "embassy+lvgl",
            r#"metaschema_version: "v1"
entities: []
relations: []
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (_, findings) = resolve(&inv);
        assert!(findings.iter().any(|f| f.rule == "METASCHEMA-004"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_bridge_relation_qualifies_and_resolves() {
        let root = tmp("bridge-ok");
        write_skeleton(&root);
        write_bridge(
            &root,
            "embassy+lvgl",
            r#"metaschema_version: "v1"
entities: []
relations:
  - kind: renders
    extends: depends_on
    from_kind: embassy.Task
    to_kind: lvgl.Widget
    from_spec: "specs/modules/*.toml"
    endpoints: { from: "${.from}", to: "${.to}" }
"#,
        );
        let layout = LayoutContext::discover(&root, None).unwrap();
        let inv = load_all(&layout).unwrap();
        let (kindset, findings) = resolve(&inv);
        assert!(!findings.iter().any(|f| f.severity == Severity::Error));
        let renders = kindset.relation("embassy+lvgl.renders").unwrap();
        assert_eq!(
            renders.extends,
            ResolvedExtends::Canonical("depends_on".into())
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
