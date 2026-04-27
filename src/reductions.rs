//! Layout-level reduction loader (Phase 1).
//!
//! Distinct from [`crate::reduction`], which is the prior-arc reduction
//! executor. This module owns the new layout's reduction loading: walk
//! `reductions/project.yaml`, `reductions/blocks/*.yaml`,
//! `reductions/bridges/*.yaml`, and `reductions/bridges/local/*.yaml`,
//! parse each file to a `serde_yaml::Value`, and compute the namespace
//! from the file's filesystem location.
//!
//! Phase 1 produces strings only — no `extends:` resolution, no kind
//! path-qualification, no canonical-vs-local enforcement at load time.
//! Phase 2's metaschema amendment cuts that work in.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value as JsonValue;
use tracing::{event, instrument, Level};

use crate::diagnostics::LocatedError;
use crate::layout::LayoutContext;

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
    })
}

/// Shallow walk the parsed YAML for `kind:` and `extends:` strings inside
/// the conventional `entities:` and `relations:` lists. Phase 2 will replace
/// this with proper structured parsing.
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
}
