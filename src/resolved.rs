//! Phase-2 resolved data structures.
//!
//! Sits one level above [`crate::reductions`]'s `Raw*` types: where the
//! Phase-1 loader hands back kind names as opaque strings, this module
//! holds the post-resolution form — kind paths parsed into [`KindPath`]s,
//! `extends` targets resolved to either a canonical anchor or another
//! qualified kind, and bodies kept verbatim for the executor.
//!
//! Pure data — behavior lives in `reductions::resolve`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::kinds::KindPath;
use crate::reductions::ReductionNamespace;

/// A single reduction file, post-resolution. Holds the namespace it lives
/// under, the parsed entity and relation declarations with qualified
/// kinds, and the original parsed YAML for the executor to walk.
#[derive(Debug, Clone)]
pub struct ResolvedReduction {
    pub source_path: PathBuf,
    pub namespace: ReductionNamespace,
    pub entities: Vec<ResolvedEntityDecl>,
    pub relations: Vec<ResolvedRelationDecl>,
    /// Verbatim parsed YAML body, retained so the executor can read
    /// `from`, `for_each`, `name`, `attrs`, `endpoints`, `from_spec`,
    /// and `render` without re-parsing.
    pub raw: serde_yaml::Value,
}

#[derive(Debug, Clone)]
pub struct ResolvedEntityDecl {
    /// Always qualified after resolution — block-qualified for entries
    /// from `blocks/<name>.yaml`, unqualified for entries from
    /// `project.yaml` (profile root namespace).
    pub path: KindPath,
    pub extends: ResolvedExtends,
    pub source_path: PathBuf,
    pub source_line: u32,
    /// The entity body (kind / extends / from / name / attrs / render)
    /// as a YAML value, for the executor.
    pub raw: serde_yaml::Value,
}

#[derive(Debug, Clone)]
pub struct ResolvedRelationDecl {
    pub path: KindPath,
    pub extends: ResolvedExtends,
    pub from_kind: KindPath,
    pub to_kind: KindPath,
    pub source_path: PathBuf,
    pub source_line: u32,
    pub raw: serde_yaml::Value,
}

/// Resolved `extends` target. Either a canonical-vocabulary string (root
/// kind for entities, relation kind for relations) or another qualified
/// kind that itself extends to a canonical anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedExtends {
    /// Canonical anchor — a member of the closed root-kind enum (for
    /// entities) or relation-kind enum (for relations).
    Canonical(String),
    /// A previously-resolved qualified kind in the effective set. The
    /// resolver verifies this chain reaches a canonical anchor.
    Qualified(KindPath),
}

/// The effective open-vocabulary kind set computed at load.
///
/// `canonical_roots` and `canonical_relations` are the closed v1 enums.
/// `entities` and `relations` accumulate every qualified kind contributed
/// by the project's reductions (profile + blocks + bridges), keyed by
/// the qualified path string for fast lookup.
#[derive(Debug, Clone, Default)]
pub struct EffectiveKindSet {
    pub canonical_roots: BTreeSet<String>,
    pub canonical_relations: BTreeSet<String>,
    pub entities: std::collections::BTreeMap<String, ResolvedEntityDecl>,
    pub relations: std::collections::BTreeMap<String, ResolvedRelationDecl>,
}

impl EffectiveKindSet {
    /// Look up an entity by its qualified path string (e.g., `embassy.Task`
    /// or `Module`).
    pub fn entity(&self, path: &str) -> Option<&ResolvedEntityDecl> {
        self.entities.get(path)
    }

    /// Look up a relation by its qualified path string.
    pub fn relation(&self, path: &str) -> Option<&ResolvedRelationDecl> {
        self.relations.get(path)
    }

    /// Walk an entity's `extends` chain to its canonical root.
    /// Returns the canonical-root string if the chain anchors, or `None`
    /// if the chain is broken (resolver should have rejected such cases
    /// at load time, so `None` is an invariant violation in practice).
    pub fn entity_canonical_root(&self, path: &str) -> Option<String> {
        if self.canonical_roots.contains(path) {
            return Some(path.to_string());
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut cursor = path.to_string();
        loop {
            if !seen.insert(cursor.clone()) {
                return None;
            }
            let entity = self.entities.get(&cursor)?;
            match &entity.extends {
                ResolvedExtends::Canonical(s) => return Some(s.clone()),
                ResolvedExtends::Qualified(p) => {
                    cursor = p.to_string();
                }
            }
        }
    }

    /// Walk a relation's `extends` chain to its canonical relation kind.
    pub fn relation_canonical(&self, path: &str) -> Option<String> {
        if self.canonical_relations.contains(path) {
            return Some(path.to_string());
        }
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut cursor = path.to_string();
        loop {
            if !seen.insert(cursor.clone()) {
                return None;
            }
            let relation = self.relations.get(&cursor)?;
            match &relation.extends {
                ResolvedExtends::Canonical(s) => return Some(s.clone()),
                ResolvedExtends::Qualified(p) => {
                    cursor = p.to_string();
                }
            }
        }
    }
}
