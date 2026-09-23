//! `derive` — files derived per module from the profile's `derived_files`
//! rules, written by the analyzer and recorded as lineage rows marked
//! `derived`.
//!
//! A derived file is fully determined by the module document (a route
//! shim that re-exports the module body is the motivating case). Writing
//! it from prose in the non-deterministic generator has three defects no
//! early gate catches: a wrong form that only the batch build rejects,
//! two modules writing one path with no check, and a stale file left in
//! place after the module moved. So the file is derived here, from
//! profile data, deterministically.
//!
//! Per run, in order:
//!
//! 1. **Rule set.** The inlined `profile.yaml`'s `derived_files` list. If
//!    it is absent and the plugin's copy of the same profile has one, the
//!    inlined profile is stale and the run is refused with the block to
//!    paste — a silent no-op would leave a tree with no derived files
//!    while every gate passes. Neither has one → no-op, nothing printed.
//!    Rules are matched in declared order, first match wins; an earlier
//!    rule whose `when` set is a subset of a later rule's makes the later
//!    rule unreachable and the set is refused before any work.
//! 2. **Targets.** For every module with a body on disk, the first
//!    matching rule's `path` evaluated over the module document and the
//!    template body with `{source_relative}` and `{project_name}`
//!    replaced. A module with no body is `skipped` — a derived file whose
//!    import does not exist would fail the whole tree. Every target is
//!    computed before anything is written: two modules on one target
//!    (case folded) or two targets in one directory from one `exclusive`
//!    group are refused.
//! 3. **Write policy** — a three-way compare of desired bytes **D**, disk
//!    bytes **F** and the recorded hash **R**: no file → `wrote`; D == F →
//!    `kept` (whatever R holds); F == R and D ≠ F → `wrote`; F ≠ R →
//!    `edited`, not overwritten; unrecorded and F ≠ D → `conflict`, not
//!    overwritten. `--accept-edits` overwrites the last two.
//! 4. **Stale rows.** A row with `derived = true` whose path no rule
//!    produces any more: file bytes match the record → file and row
//!    removed (`deleted`); file absent → row removed (`deleted`); bytes
//!    differ → both kept, reported `edited` (`--accept-edits` deletes).
//!
//! Without `--write` every line is a preview and nothing is written or
//! recorded. Nothing here names a role, a file name, a directory or a
//! framework: the rule set is profile data and the import-specifier
//! grammar is the language adapter's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::gen_lineage::{check_ownership, rel_key, GenEntry, GenLineage};
use crate::hash::content_hash;
use crate::items::source_map::{load_source_file_override, resolve_source_path};
use crate::layout::LayoutContext;
use crate::manifest::{DerivedRule, Profile};
use crate::path::{eval_template, render_scalar, Scope};

/// Body tokens. Exact-string replacement, then a post-check that neither
/// literal survives; no brace scanner (`export { default }` would read as
/// a token).
const TOKEN_SOURCE_RELATIVE: &str = "{source_relative}";
const TOKEN_PROJECT_NAME: &str = "{project_name}";

#[derive(Debug, Clone, Copy, Default)]
pub struct DeriveOptions {
    /// Write files and record rows. Without it the run is a preview.
    pub write: bool,
    /// Overwrite `edited` and `conflict` targets and delete stale
    /// `edited` files. Never passed by the codegen skill.
    pub accept_edits: bool,
}

/// One line of the run's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Wrote,
    Kept,
    Edited,
    Conflict,
    Deleted,
    Skipped,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Wrote => "wrote",
            Outcome::Kept => "kept",
            Outcome::Edited => "edited",
            Outcome::Conflict => "conflict",
            Outcome::Deleted => "deleted",
            Outcome::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeriveLine {
    pub outcome: Outcome,
    /// Project-root-relative target path; the module name for `skipped`.
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl DeriveLine {
    /// `<outcome> <path>[: <note>]` — the one-line form the CLI prints.
    pub fn render(&self) -> String {
        match &self.note {
            Some(n) => format!("{} {}: {n}", self.outcome.as_str(), self.path),
            None => format!("{} {}", self.outcome.as_str(), self.path),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct DeriveReport {
    /// Sorted by path for a deterministic report.
    pub lines: Vec<DeriveLine>,
}

impl DeriveReport {
    /// Lines whose target was held back: an `edited` or `conflict` path
    /// the run did not overwrite. The CLI exits non-zero when any exist,
    /// so a non-LLM caller learns of a skipped file without reading stdout.
    pub fn held(&self) -> usize {
        self.lines
            .iter()
            .filter(|l| matches!(l.outcome, Outcome::Edited | Outcome::Conflict))
            .count()
    }

    fn push(&mut self, outcome: Outcome, path: impl Into<String>, note: Option<String>) {
        self.lines.push(DeriveLine {
            outcome,
            path: path.into(),
            note,
        });
    }
}

/// A computed target, before the write policy is applied.
struct Target {
    module: String,
    rule_index: usize,
    /// Project-root-relative, forward slashes.
    rel: String,
    group: Option<String>,
    bytes: Vec<u8>,
}

/// Run `derive`. `resolve_plugin` is called only when the inlined profile
/// declares no rules (to detect a stale inlined profile); it may fail
/// with the usual plugin-root error.
pub fn run_derive(
    layout: &LayoutContext,
    resolve_plugin: &dyn Fn() -> Result<PathBuf>,
    opts: DeriveOptions,
) -> Result<DeriveReport> {
    let mut report = DeriveReport::default();

    // 1. Rule set, with the stale-inlined-profile refusal.
    let inlined = Profile::load_inlined(&layout.specdev_root)?;
    let rules: Vec<DerivedRule> = match inlined {
        Some(p) if !p.derived_files.is_empty() => p.derived_files,
        _ => {
            let plugin = resolve_plugin()?;
            let plugin_profile = plugin
                .join("profiles")
                .join(&layout.spec.project.profile)
                .join("profile.yaml");
            if plugin_profile.is_file() {
                let shipped = Profile::load(&plugin_profile)?;
                if !shipped.derived_files.is_empty() {
                    refuse_stale_inlined_profile(layout, &plugin_profile, &shipped.derived_files)?;
                }
            }
            return Ok(report);
        }
    };
    refuse_unreachable_rules(&rules)?;

    // Template bodies — read before any work so a missing file is an
    // error before any write.
    let mut templates: Vec<String> = Vec::with_capacity(rules.len());
    for rule in &rules {
        let path = layout.specdev_root.join(&rule.template);
        let body = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "derive: reading template {} (referenced from profile.yaml derived_files rule `{}`)",
                path.display(),
                describe_when(&rule.when)
            )
        })?;
        templates.push(body);
    }

    let language = layout.spec.project.language.as_str();
    let adapter = crate::lang::adapter_for(language)
        .ok_or_else(|| anyhow!("derive: unknown language `{language}`"))?;
    let project_root = layout
        .project_root
        .canonicalize()
        .with_context(|| format!("canonicalizing {}", layout.project_root.display()))?;
    let source_root = layout.spec.paths.source.as_str();
    let project_name = layout.spec.project.name.as_str();

    // 2. Targets for every module, before any write.
    let module_names = crate::prompt::list_module_names(layout)?;
    let mut targets: Vec<Target> = Vec::new();
    for name in &module_names {
        let module_path = layout.modules_dir.join(format!("{name}.toml"));
        let doc = crate::reduction::load_toml_as_json(&module_path)?;
        let Some(rule_index) = rules.iter().position(|r| rule_matches(r, &doc)) else {
            continue;
        };
        let rule = &rules[rule_index];

        let source_file = load_source_file_override(&module_path);
        let Some(body_abs) = resolve_source_path(
            name,
            source_file.as_deref(),
            adapter.as_ref(),
            &project_root,
            source_root,
        ) else {
            report.push(Outcome::Skipped, name.clone(), Some("no body".to_string()));
            continue;
        };
        let body_rel = body_abs
            .strip_prefix(&project_root)
            .map(rel_key)
            .unwrap_or_else(|_| body_abs.to_string_lossy().into_owned());

        let scope = Scope {
            root: &doc,
            current: Some(&doc),
            parent: None,
        };
        let evaluated = eval_template(&rule.path, &scope).with_context(|| {
            format!(
                "derive: module `{name}` matched derived_files rule `{}` but its path \
                 template `{}` could not be evaluated over the module document",
                describe_when(&rule.when),
                rule.path
            )
        })?;
        let rel = normalize_target(&evaluated, name)?;

        let from_dir = Path::new(&rel).parent().unwrap_or_else(|| Path::new(""));
        let specifier = adapter.import_specifier(from_dir, Path::new(&body_rel));
        let body = templates[rule_index]
            .replace(TOKEN_SOURCE_RELATIVE, &specifier)
            .replace(TOKEN_PROJECT_NAME, project_name);
        for token in [TOKEN_SOURCE_RELATIVE, TOKEN_PROJECT_NAME] {
            if body.contains(token) {
                bail!(
                    "derive: template `{}` still contains the literal `{token}` after \
                     substitution (rendered for module `{name}` at `{rel}`)",
                    rule.template
                );
            }
        }

        targets.push(Target {
            module: name.clone(),
            rule_index,
            rel,
            group: rule.exclusive.clone(),
            bytes: body.into_bytes(),
        });
    }
    refuse_collisions(&targets, &rules)?;

    // 3. Write policy, per target, sorted by path.
    targets.sort_by(|a, b| a.rel.cmp(&b.rel));
    let mut lineage = GenLineage::load(&layout.specdev_root)?;
    let generated_at = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let mut input_hashes: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut lineage_dirty = false;
    let produced: BTreeSet<&str> = targets.iter().map(|t| t.rel.as_str()).collect();

    for target in &targets {
        let abs = project_root.join(&target.rel);
        if abs.is_dir() {
            bail!(
                "derive: target `{}` for module `{}` is a directory; remove or rename it",
                target.rel,
                target.module
            );
        }
        let existing = lineage.files.get(&target.rel);
        if let Some(old) = check_ownership(
            existing,
            &target.rel,
            Some(&target.module),
            &module_names,
            "derive",
        )? {
            tracing::warn!(
                target: "specdev_analyzer::derive",
                "derive: `{}` was recorded for module `{old}`, which no longer exists in \
                 specs/modules/; reassigned to module `{}`",
                target.rel,
                target.module
            );
        }
        let recorded_hash = existing.map(|e| e.content_hash.clone());
        let recorded_at = existing.map(|e| e.generated_at.clone());
        let desired = &target.bytes;
        let on_disk = match std::fs::read(&abs) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => bail!("derive: reading `{}`: {e}", target.rel),
        };

        // The matrix. `D == F` is tested first: a hand edit that equals
        // the new desired bytes is not an edited file.
        let (outcome, note) = match (&on_disk, &recorded_hash) {
            (None, _) => (Outcome::Wrote, None),
            (Some(f), _) if f == desired => (Outcome::Kept, None),
            (Some(f), Some(r)) if &content_hash(f) == r => (Outcome::Wrote, None),
            (Some(_), Some(_)) => (Outcome::Edited, None),
            (Some(_), None) => (
                Outcome::Conflict,
                Some("unrecorded file at target".to_string()),
            ),
        };
        let overwrite = match outcome {
            Outcome::Wrote => true,
            Outcome::Edited | Outcome::Conflict => opts.accept_edits,
            _ => false,
        };
        let (outcome, note) = if overwrite && outcome != Outcome::Wrote {
            (Outcome::Wrote, None)
        } else {
            (outcome, note)
        };

        if opts.write && (outcome == Outcome::Wrote || outcome == Outcome::Kept) {
            if outcome == Outcome::Wrote {
                if let Some(parent) = abs.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("derive: creating directory {}", parent.display())
                    })?;
                }
                std::fs::write(&abs, desired)
                    .with_context(|| format!("derive: writing `{}`", target.rel))?;
            }
            let input_hash = input_hashes
                .entry(target.module.clone())
                .or_insert_with(|| module_input_hash(layout, &target.module))
                .clone();
            // `kept` keeps its `generated_at`: a committed lineage file
            // must not churn when nothing changed.
            let generated_at = match (outcome, recorded_at) {
                (Outcome::Kept, Some(at)) => at,
                _ => generated_at.clone(),
            };
            lineage.files.insert(
                target.rel.clone(),
                GenEntry {
                    module: Some(target.module.clone()),
                    generated_at,
                    content_hash: content_hash(desired),
                    input_hash,
                    derived: true,
                },
            );
            lineage_dirty = true;
        }
        let _ = target.rule_index;
        report.push(outcome, target.rel.clone(), note);
    }

    // 4. Stale derived rows.
    //
    // A stale key can name the same on-disk file as a produced target on a
    // case-insensitive file system (an `app_path` that changed only in
    // letter case): the produced target was just kept or written, so the
    // stale row is removed but the file is not — deleting it would remove
    // the live shim until the next run. Identity is by canonical path.
    let produced_canonical: BTreeSet<PathBuf> = targets
        .iter()
        .filter_map(|t| project_root.join(&t.rel).canonicalize().ok())
        .collect();
    let stale: Vec<(String, GenEntry)> = lineage
        .files
        .iter()
        .filter(|(k, e)| e.derived && !produced.contains(k.as_str()))
        .map(|(k, e)| (k.clone(), e.clone()))
        .collect();
    for (key, entry) in stale {
        let abs = project_root.join(&key);
        let on_disk = match std::fs::read(&abs) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => bail!("derive: reading `{key}`: {e}"),
        };
        let same_file_as_produced = on_disk.is_some()
            && abs
                .canonicalize()
                .is_ok_and(|c| produced_canonical.contains(&c));
        if same_file_as_produced {
            if opts.write {
                lineage.files.remove(&key);
                lineage_dirty = true;
            }
            report.push(
                Outcome::Deleted,
                key,
                Some("row only; the file is a produced target under another spelling".to_string()),
            );
            continue;
        }
        let matches = on_disk
            .as_ref()
            .is_some_and(|f| content_hash(f) == entry.content_hash);
        let delete = on_disk.is_none() || matches || opts.accept_edits;
        if delete {
            if opts.write {
                if on_disk.is_some() {
                    std::fs::remove_file(&abs)
                        .with_context(|| format!("derive: deleting stale `{key}`"))?;
                }
                lineage.files.remove(&key);
                lineage_dirty = true;
            }
            report.push(Outcome::Deleted, key, None);
        } else {
            report.push(
                Outcome::Edited,
                key,
                Some(
                    "stale derived file kept because of a hand edit; remove it or run \
                     `derive --write --accept-edits`"
                        .to_string(),
                ),
            );
        }
    }

    if opts.write && lineage_dirty {
        lineage.save(&layout.specdev_root)?;
    }
    report.lines.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(report)
}

/// The module's assembled-prompt hash — the same digest `gen-record
/// --module` records, so the body row and the derived rows agree. A
/// module whose prompt cannot be assembled records no hash (fail-safe;
/// the body row decides the verdict anyway).
fn module_input_hash(layout: &LayoutContext, module: &str) -> Option<String> {
    match crate::prompt::module_input_hash_quiet(layout, module) {
        Ok(h) => Some(h),
        Err(e) => {
            tracing::warn!(
                target: "specdev_analyzer::derive",
                "could not assemble the prompt for module `{module}` to record its \
                 input_hash ({e:#})"
            );
            None
        }
    }
}

/// `when` matches when every key is present on the document and renders
/// to the same scalar. An absent key matches nothing; `{}` matches all.
fn rule_matches(rule: &DerivedRule, doc: &Value) -> bool {
    rule.when.iter().all(|(key, want)| {
        doc.get(key)
            .is_some_and(|have| render_scalar(have) == render_scalar(want))
    })
}

/// `(key, rendered value)` pairs — the set the subset test runs over.
fn when_set(rule: &DerivedRule) -> BTreeSet<(String, String)> {
    rule.when
        .iter()
        .map(|(k, v)| (k.clone(), render_scalar(v)))
        .collect()
}

fn describe_when(when: &BTreeMap<String, Value>) -> String {
    if when.is_empty() {
        return "{}".to_string();
    }
    let parts: Vec<String> = when
        .iter()
        .map(|(k, v)| format!("{k}: {}", render_scalar(v)))
        .collect();
    format!("{{ {} }}", parts.join(", "))
}

/// First match wins, so an earlier rule whose `when` set is a subset of a
/// later rule's leaves the later rule unreachable. Refused, naming both.
/// Single-rule shadowing only: several earlier rules that together cover
/// a closed enum are not detected.
fn refuse_unreachable_rules(rules: &[DerivedRule]) -> Result<()> {
    for i in 0..rules.len() {
        let earlier = when_set(&rules[i]);
        for j in (i + 1)..rules.len() {
            let later = when_set(&rules[j]);
            if earlier.is_subset(&later) {
                bail!(
                    "profile.yaml derived_files: rule {} `{}` is listed before rule {} `{}` \
                     and matches every module the later rule matches, so the later rule \
                     is unreachable. Order rules from most specific to least specific.",
                    i + 1,
                    describe_when(&rules[i].when),
                    j + 1,
                    describe_when(&rules[j].when)
                );
            }
        }
    }
    Ok(())
}

/// Collapse duplicate slashes, drop a leading `./`, and refuse anything
/// that is not a relative path inside the project root.
fn normalize_target(evaluated: &str, module: &str) -> Result<String> {
    let mut collapsed = String::with_capacity(evaluated.len());
    let mut prev_slash = false;
    for ch in evaluated.chars() {
        if ch == '/' {
            if prev_slash {
                continue;
            }
            prev_slash = true;
        } else {
            prev_slash = false;
        }
        collapsed.push(ch);
    }
    let trimmed = collapsed.strip_prefix("./").unwrap_or(&collapsed);
    let path = Path::new(trimmed);
    let escapes = path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if escapes || trimmed.trim().is_empty() || trimmed.ends_with('/') {
        bail!(
            "derive: target `{evaluated}` for module `{module}` must be a relative file \
             path inside the project root (no leading `/`, no `..`)"
        );
    }
    Ok(rel_key(path))
}

/// Same target from two modules (case folded — two spellings are one
/// directory on APFS and NTFS) and two targets in one directory from one
/// `exclusive` group are both refused, naming both modules.
fn refuse_collisions(targets: &[Target], rules: &[DerivedRule]) -> Result<()> {
    let mut by_path: BTreeMap<String, &Target> = BTreeMap::new();
    for t in targets {
        if let Some(other) = by_path.insert(t.rel.to_lowercase(), t) {
            bail!(
                "derive: modules `{}` and `{}` both derive `{}` (`{}` and `{}` are one \
                 path on a case-insensitive file system); one target has one module",
                other.module,
                t.module,
                t.rel,
                other.rel,
                t.rel
            );
        }
    }
    let mut by_group: BTreeMap<(String, String), &Target> = BTreeMap::new();
    for t in targets {
        let Some(group) = &t.group else {
            continue;
        };
        let dir = Path::new(&t.rel)
            .parent()
            .map(|p| rel_key(p).to_lowercase())
            .unwrap_or_default();
        if let Some(other) = by_group.insert((dir.clone(), group.clone()), t) {
            bail!(
                "derive: modules `{}` (`{}`, rule `{}`) and `{}` (`{}`, rule `{}`) both \
                 derive a file in `{dir}/` from exclusive group `{group}`; one such file \
                 per directory",
                other.module,
                other.rel,
                describe_when(&rules[other.rule_index].when),
                t.module,
                t.rel,
                describe_when(&rules[t.rule_index].when)
            );
        }
    }
    Ok(())
}

/// The inlined profile declares no rules but the plugin's copy does: the
/// inlined profile is stale. Refuse with the block to paste.
fn refuse_stale_inlined_profile(
    layout: &LayoutContext,
    plugin_profile: &Path,
    rules: &[DerivedRule],
) -> Result<()> {
    let block = rules
        .iter()
        .map(|r| {
            let mut line = format!(
                "  - when: {}\n    path: \"{}\"\n    template: {}",
                describe_when(&r.when),
                r.path,
                r.template
            );
            if let Some(g) = &r.exclusive {
                line.push_str(&format!("\n    exclusive: {g}"));
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n");
    bail!(
        "derive: {} declares no `derived_files:` but the plugin's profile at {} does. \
         The inlined profile is stale; without the rules this tree gets no derived \
         files while every gate passes. Paste this block into the inlined \
         profile.yaml (or re-bootstrap) and re-run:\n\nderived_files:\n{block}\n\n\
         The templates it names must exist under {}/templates/.",
        layout.specdev_root.join("profile.yaml").display(),
        plugin_profile.display(),
        layout.specdev_root.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(when: Value, path: &str, exclusive: Option<&str>) -> DerivedRule {
        let when: BTreeMap<String, Value> = serde_json::from_value(when).unwrap();
        DerivedRule {
            when,
            path: path.to_string(),
            template: "templates/t.template".to_string(),
            exclusive: exclusive.map(str::to_string),
        }
    }

    #[test]
    fn when_compares_rendered_scalars_and_absent_key_matches_nothing() {
        let r = rule(json!({"kind": "x", "flag": true}), "p", None);
        assert!(rule_matches(&r, &json!({"kind": "x", "flag": true})));
        assert!(!rule_matches(&r, &json!({"kind": "x", "flag": false})));
        assert!(!rule_matches(&r, &json!({"kind": "x"})));
        // A string "true" renders like a boolean true.
        assert!(rule_matches(&r, &json!({"kind": "x", "flag": "true"})));
        let all = rule(json!({}), "p", None);
        assert!(rule_matches(&all, &json!({"anything": 1})));
    }

    #[test]
    fn earlier_subset_rule_makes_later_rule_unreachable() {
        let general_first = vec![
            rule(json!({"kind": "a"}), "p1", None),
            rule(json!({"kind": "a", "sub": "s"}), "p2", None),
        ];
        let err = refuse_unreachable_rules(&general_first).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("rule 1"), "{msg}");
        assert!(msg.contains("rule 2"), "{msg}");
        assert!(msg.contains("unreachable"), "{msg}");

        let specific_first = vec![
            rule(json!({"kind": "a", "sub": "s"}), "p1", None),
            rule(json!({"kind": "a", "sub": "t"}), "p2", None),
            rule(json!({"kind": "a"}), "p3", None),
            rule(json!({"kind": "b"}), "p4", None),
        ];
        refuse_unreachable_rules(&specific_first).unwrap();

        // `{}` first shadows everything; identical sets shadow too.
        let empty_first = vec![
            rule(json!({}), "p1", None),
            rule(json!({"kind": "a"}), "p2", None),
        ];
        assert!(refuse_unreachable_rules(&empty_first).is_err());
        let twins = vec![
            rule(json!({"kind": "a"}), "p1", None),
            rule(json!({"kind": "a"}), "p2", None),
        ];
        assert!(refuse_unreachable_rules(&twins).is_err());
    }

    #[test]
    fn target_normalization_collapses_slashes_and_refuses_escapes() {
        assert_eq!(normalize_target("app//x.tsx", "M").unwrap(), "app/x.tsx");
        assert_eq!(normalize_target("./app/x.tsx", "M").unwrap(), "app/x.tsx");
        assert_eq!(
            normalize_target("app/(g)/[id]/x.tsx", "M").unwrap(),
            "app/(g)/[id]/x.tsx"
        );
        assert!(normalize_target("/etc/x", "M").is_err());
        assert!(normalize_target("app/../x", "M").is_err());
        assert!(normalize_target("app/", "M").is_err());
        assert!(normalize_target("", "M").is_err());
    }

    fn target(module: &str, rel: &str, rule_index: usize, group: Option<&str>) -> Target {
        Target {
            module: module.to_string(),
            rule_index,
            rel: rel.to_string(),
            group: group.map(str::to_string),
            bytes: Vec::new(),
        }
    }

    #[test]
    fn collisions_fold_case_and_honor_exclusive_groups() {
        let rules = vec![
            rule(json!({"kind": "p"}), "x", Some("segment")),
            rule(json!({"kind": "r"}), "y", Some("segment")),
            rule(json!({"kind": "l"}), "z", None),
        ];
        // Same path, different case → refused naming both modules.
        let err = refuse_collisions(
            &[
                target("A", "app/Events/page.tsx", 0, None),
                target("B", "app/events/page.tsx", 0, None),
            ],
            &rules,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("`A`") && msg.contains("`B`"), "{msg}");

        // One directory, two files from one exclusive group → refused.
        let err = refuse_collisions(
            &[
                target("P", "app/c/page.tsx", 0, Some("segment")),
                target("R", "app/c/route.ts", 1, Some("segment")),
            ],
            &rules,
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("exclusive group `segment`"), "{msg}");
        assert!(msg.contains("`P`") && msg.contains("`R`"), "{msg}");

        // One directory, one exclusive file plus a non-exclusive one → fine;
        // two exclusive files in different directories → fine.
        refuse_collisions(
            &[
                target("P", "app/c/page.tsx", 0, Some("segment")),
                target("L", "app/c/layout.tsx", 2, None),
                target("R", "app/api/route.ts", 1, Some("segment")),
            ],
            &rules,
        )
        .unwrap();
    }
}
