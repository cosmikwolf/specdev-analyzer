//! `migrate`'s format step: a `spec_version = "1.x"` project to `"2.0"`.
//!
//! Every change is planned in memory first; a refusal stops the step before
//! any byte is written, and `--dry-run` stops after the plan. `spec_version`
//! is written last, so an interrupted run leaves a `1.x` tree that the same
//! command finishes. On a `2.x` tree the step does nothing.
//!
//! The steps, in order:
//!
//! 1. Split the combined decision, requirement and open-question files into
//!    one file per entry, each keeping its bytes; a tombstone replaces each
//!    combined file ([`crate::entries::plan_split`]).
//! 2. Module specs lose their test items (each item's `verifies` moves to
//!    the requirement as `verified_by`) and the `external_deps` table, which
//!    restates the manifest.
//! 3. Modules whose source does not resolve are reported, and marked
//!    `planned = true` only under `--plan-unresolved`. Modules whose source
//!    holds only tests are reported.
//! 4. A requirement that is not planned (set, or derived: at least one
//!    satisfier and every satisfier planned) and has no `verified_by` gets
//!    `test_pending = true` — after step 3, so its flags count.
//! 5. Profile files: a file still equal to what bootstrap recorded takes the
//!    plugin's bytes and a new record. `spec.toml`, `domain.toml` and the
//!    reductions otherwise get precise in-place edits and keep their record.
//!    A schema with no record takes the plugin's bytes; a schema edited
//!    since its record refuses the step.
//! 6. `gen-lineage.toml` and the profile's inlined `codegen/` go.
//! 7. `lineage.toml`: `[meta].plugin_version` becomes `bootstrapped_with`;
//!    records of moved and removed files go.
//! 8. `spec.toml`: `spec_version = "2.0"` and `max_parallel_agents = 1`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use toml_edit::{value, Array, DocumentMut, Item, Value};

use crate::bootstrap::{self, BootstrapInputs};
use crate::entries::{self, EntryKind, EntryLayout, EntryStore};
use crate::items::source_map::{resolve_source_path, source_file_override};
use crate::layout::LayoutContext;
use crate::lineage::Lineage;

/// The format the step writes.
pub const TARGET_VERSION: &str = "2.0";

/// The rules the format renames: `(old, new)`.
const RENAMED_RULES: &[(&str, &str)] = &[("codegen_unblocked", "implementation_unblocked")];

/// The rules whose severity becomes `error`.
const ERROR_RULES: &[&str] = &["requirements_have_satisfiers", "implementation_unblocked"];

/// Graph checks the format drops: their relation came from spec test
/// items, which the format does not have.
const DROPPED_RULES: &[&str] = &[
    "test_requirements_have_verifiers",
    "feature_unique_names",
    "workspace_dependencies_unique",
];

/// Options for the format step.
#[derive(Debug, Clone, Copy, Default)]
pub struct FormatOptions {
    pub dry_run: bool,
    pub plan_unresolved: bool,
}

/// What the step did, or would do.
#[derive(Debug, Default)]
pub struct FormatReport {
    /// The tree is already in the target format; nothing was planned.
    pub already_current: bool,
    /// One line per change, in order.
    pub lines: Vec<String>,
    /// Modules with no resolvable source and no `planned = true`.
    pub unresolved: Vec<String>,
    /// Modules marked `planned = true` by `--plan-unresolved`.
    pub planned: Vec<String>,
    /// Modules whose source file holds only tests.
    pub tests_only: Vec<String>,
    /// Workspace members moved with the root (their project roots).
    pub members: Vec<PathBuf>,
    /// The root was already at 2.x: only folds (and members) moved.
    pub folded: bool,
    /// Unrecorded files replaced with the plugin's bytes (no baseline).
    pub replaced_unrecorded: Vec<String>,
    /// Field values the 2.0 field cuts removed (each is one of `lines`).
    pub removed_values: usize,
}

/// Pending file changes, keyed by project-relative path.
struct Overlay {
    root: PathBuf,
    writes: BTreeMap<PathBuf, Vec<u8>>,
    deletes: BTreeSet<PathBuf>,
    delete_dirs: BTreeSet<PathBuf>,
}

impl Overlay {
    fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            writes: BTreeMap::new(),
            deletes: BTreeSet::new(),
            delete_dirs: BTreeSet::new(),
        }
    }

    fn read(&self, rel: &Path) -> Option<Vec<u8>> {
        if let Some(b) = self.writes.get(rel) {
            return Some(b.clone());
        }
        if self.deletes.contains(rel) || self.delete_dirs.iter().any(|d| rel.starts_with(d)) {
            return None;
        }
        std::fs::read(self.root.join(rel)).ok()
    }

    fn write(&mut self, rel: &Path, bytes: Vec<u8>) {
        self.deletes.remove(rel);
        self.writes.insert(rel.to_path_buf(), bytes);
    }

    /// The `*.toml` files in project-relative `dir`, on disk or pending.
    fn toml_files(&self, dir: &Path) -> Vec<PathBuf> {
        let mut out: BTreeSet<PathBuf> = BTreeSet::new();
        if let Ok(rd) = std::fs::read_dir(self.root.join(dir)) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file()
                    && p.extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("toml"))
                {
                    out.insert(dir.join(e.file_name()));
                }
            }
        }
        for p in self.writes.keys() {
            if p.parent() == Some(dir)
                && p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("toml"))
            {
                out.insert(p.clone());
            }
        }
        out.into_iter().filter(|p| self.read(p).is_some()).collect()
    }

    /// Apply every change; `last` is written after everything else.
    fn apply(&self, lineage: Option<(&Lineage, &Path)>, last: &Path) -> Result<()> {
        for (rel, bytes) in &self.writes {
            if rel != last {
                write(&self.root.join(rel), bytes)?;
            }
        }
        for rel in &self.deletes {
            let p = self.root.join(rel);
            if p.is_file() {
                std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
            }
        }
        for rel in &self.delete_dirs {
            let p = self.root.join(rel);
            if p.is_dir() {
                std::fs::remove_dir_all(&p).with_context(|| format!("removing {}", p.display()))?;
            }
        }
        if let Some((l, specdev_root)) = lineage {
            l.save(specdev_root)?;
        }
        if let Some(bytes) = self.writes.get(last) {
            write(&self.root.join(last), bytes)?;
        }
        Ok(())
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

fn rel_to(root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(root).unwrap_or(p).to_path_buf()
}

fn slash(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn utf8(bytes: Vec<u8>, rel: &Path) -> Result<String> {
    String::from_utf8(bytes).map_err(|_| anyhow!("{} is not valid UTF-8", rel.display()))
}

fn string_list(item: Option<&Item>) -> Vec<String> {
    item.and_then(Item::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One module spec as the step reads it.
struct ModuleSpec {
    rel: PathBuf,
    name: String,
    doc: DocumentMut,
    changed: bool,
    satisfies: Vec<String>,
    planned: bool,
}

/// One project's planned changes, not yet written.
struct Plan {
    ov: Overlay,
    /// `None` for a fold, which leaves `lineage.toml` alone.
    lineage: Option<Lineage>,
    specdev_root: PathBuf,
    report: FormatReport,
}

/// Run the format step on `layout`'s project, against the plugin at
/// `plugin_root` (the source of the 2.0 profile files). A workspace root
/// moves with every member it lists: each is planned first,
/// a refusal in any of them stops the run before a byte is written, and
/// then the members are written, the root last. In a member the step
/// refuses and names the root.
pub fn run(
    layout: &LayoutContext,
    plugin_root: &Path,
    opts: FormatOptions,
) -> Result<FormatReport> {
    if let Some(root_spec) = crate::layout::find_workspace_root_spec(&layout.project_root)? {
        if layout.spec.format_major() >= 2 {
            // A member at the root's major may still fold its own legacy content.
            return finish(vec![(None, fold_plan(layout, opts)?)], layout, opts, true);
        }
        let root = root_spec.parent().unwrap_or(&root_spec);
        bail!(
            "[MIGRATE-015] {} is a member of the workspace at {}. A workspace moves to {TARGET_VERSION} as one: \
             run `specdev-analyzer migrate` in {}. Nothing was changed.",
            layout.project_root.display(),
            root.display(),
            root.display()
        );
    }
    let root_is_current = layout.spec.format_major() >= 2;
    let mut plans: Vec<(Option<String>, Option<Plan>)> = Vec::new();
    if let Some(ws) = &layout.spec.workspace {
        for m in &ws.members {
            let dir = layout.project_root.join(&m.path);
            // A member may be at another major than the root (an interrupted
            // run, a 1.x crate added): load it without that check.
            let member = LayoutContext::discover_unchecked(&dir)
                .with_context(|| format!("loading workspace member {}", m.path))?;
            let plan = if member.spec.format_major() >= 2 {
                fold_plan(&member, opts)
            } else {
                plan_one(&member, plugin_root, opts).map(Some)
            }
            .with_context(|| format!("workspace member {} (nothing was changed)", m.path))?;
            plans.push((Some(m.path.clone()), plan));
        }
    }
    let root_plan = if root_is_current {
        fold_plan(layout, opts)?
    } else {
        Some(plan_one(layout, plugin_root, opts)?)
    };
    plans.push((None, root_plan));
    finish(plans, layout, opts, root_is_current)
}

/// Apply every plan (members first, the root last) and merge the reports.
fn finish(
    plans: Vec<(Option<String>, Option<Plan>)>,
    layout: &LayoutContext,
    opts: FormatOptions,
    root_is_current: bool,
) -> Result<FormatReport> {
    let plans: Vec<(Option<String>, Plan)> = plans
        .into_iter()
        .filter_map(|(m, p)| p.map(|p| (m, p)))
        .collect();
    if plans.is_empty() {
        return Ok(FormatReport {
            already_current: true,
            ..Default::default()
        });
    }
    if !opts.dry_run {
        for (_, plan) in &plans {
            plan.ov.apply(
                plan.lineage
                    .as_ref()
                    .map(|l| (l, plan.specdev_root.as_path())),
                Path::new("spec.toml"),
            )?;
        }
    }
    let mut report = FormatReport {
        folded: root_is_current,
        ..Default::default()
    };
    for (member, plan) in plans {
        let tag = member
            .as_ref()
            .map(|m| format!("[{m}] "))
            .unwrap_or_default();
        let moved = plan.lineage.is_some();
        report.removed_values += plan.report.removed_values;
        report
            .lines
            .extend(plan.report.lines.into_iter().map(|l| format!("{tag}{l}")));
        report.unresolved.extend(
            plan.report
                .unresolved
                .into_iter()
                .map(|l| format!("{tag}{l}")),
        );
        report
            .planned
            .extend(plan.report.planned.into_iter().map(|l| format!("{tag}{l}")));
        report.tests_only.extend(
            plan.report
                .tests_only
                .into_iter()
                .map(|l| format!("{tag}{l}")),
        );
        report.replaced_unrecorded.extend(
            plan.report
                .replaced_unrecorded
                .into_iter()
                .map(|l| format!("{tag}{l}")),
        );
        if let (Some(m), true) = (member, moved) {
            report.members.push(layout.project_root.join(m));
        }
    }
    Ok(report)
}

/// A 2.0 tree whose combined files hold entries again (a merge from an old
/// branch, an old skill) has them folded into the per-entry files: an id
/// the layout lacks is added, the same bytes are skipped, other bytes
/// refuse with both versions shown. Nothing is overwritten; each combined
/// file becomes a tombstone again. `None` when there is nothing to fold.
fn fold_plan(layout: &LayoutContext, opts: FormatOptions) -> Result<Option<Plan>> {
    if EntryStore::for_layout(layout).legacy_content().is_empty() {
        return Ok(None);
    }
    let verb = if opts.dry_run { "would " } else { "" };
    let split = entries::plan_split(&layout.specs_dir)?;
    let mut report = FormatReport::default();
    for (file, n) in &split.moved {
        report.lines.push(format!(
            "{verb}fold {} into the per-entry files ({n} added), leaving a tombstone",
            slash(&rel_to(&layout.project_root, &layout.specs_dir.join(file)))
        ));
    }
    let mut ov = Overlay::new(&layout.project_root);
    let specs_rel = rel_to(&layout.project_root, &layout.specs_dir);
    for (rel, bytes) in split.writes.into_iter().chain(split.tombstones) {
        ov.write(&specs_rel.join(rel), bytes);
    }
    Ok(Some(Plan {
        ov,
        lineage: None,
        specdev_root: layout.specdev_root.clone(),
        report,
    }))
}

/// Plan the step for one project.
fn plan_one(layout: &LayoutContext, plugin_root: &Path, opts: FormatOptions) -> Result<Plan> {
    let mut report = FormatReport::default();
    let mut lineage = Lineage::load(&layout.specdev_root)?;
    let profile = lineage
        .meta
        .as_ref()
        .map(|m| m.profile.clone())
        .ok_or_else(|| {
            anyhow!("[MIGRATE-001] no [meta] in lineage.toml; the project is pre-bootstrap")
        })?;
    let root = &layout.project_root;
    let specs_rel = rel_to(root, &layout.specs_dir);
    let specdev_rel = rel_to(root, &layout.specdev_root);
    let mut ov = Overlay::new(root);
    let verb = if opts.dry_run { "would " } else { "" };

    // 1. One file per entry.
    let split = entries::plan_split(&layout.specs_dir)?;
    for (rel, bytes) in split.writes {
        ov.write(&specs_rel.join(rel), bytes);
    }
    for (rel, bytes) in split.tombstones {
        ov.write(&specs_rel.join(rel), bytes);
    }
    for (file, n) in &split.moved {
        report.lines.push(format!(
            "{verb}split {} into one file per entry ({n} moved), leaving a tombstone",
            slash(&specs_rel.join(file))
        ));
    }

    // The project's own manifest, for the removal report's marks.
    let facts = crate::lang::adapter_for(&layout.spec.project.language)
        .and_then(|a| a.manifest_facts(root));
    let removed_count = std::cell::Cell::new(0usize);
    let report_removed =
        |file: &Path, removed: Vec<crate::field_cuts::Removed>, lines: &mut Vec<String>| {
            for r in removed {
                removed_count.set(removed_count.get() + 1);
                let mark = crate::field_cuts::mark(&r, facts.as_ref());
                lines.push(
                    crate::field_cuts::ReportLine {
                        file: slash(file),
                        removed: r,
                        mark,
                    }
                    .render(verb),
                );
            }
        };

    // 2. Module specs: test items and the fields 2.0 removes.
    let modules_rel = rel_to(root, &layout.modules_dir);
    let mut modules: Vec<ModuleSpec> = Vec::new();
    let mut verified: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for rel in ov.toml_files(&modules_rel) {
        let text = utf8(ov.read(&rel).unwrap_or_default(), &rel)?;
        let mut doc: DocumentMut = text
            .parse()
            .with_context(|| format!("parsing {}", rel.display()))?;
        let name = doc
            .get("name")
            .and_then(Item::as_str)
            .map(str::to_string)
            .or_else(|| rel.file_stem().and_then(|s| s.to_str()).map(str::to_string))
            .unwrap_or_default();
        let mut changed = false;
        let moved = take_test_items(&mut doc);
        if !moved.is_empty() {
            changed = true;
            for (item, verifies) in &moved {
                for id in verifies {
                    let names = verified.entry(id.clone()).or_default();
                    if !names.contains(item) {
                        names.push(item.clone());
                    }
                }
            }
            report.lines.push(format!(
                "{verb}remove {} test item(s) from {}; their `verifies` move to `verified_by`",
                moved.len(),
                slash(&rel)
            ));
        }
        let cut = crate::field_cuts::strip_module(&mut doc);
        if !cut.is_empty() {
            changed = true;
            report_removed(&rel, cut, &mut report.lines);
        }
        let satisfies = string_list(doc.get("satisfies"));
        let planned = doc.get("planned").and_then(Item::as_bool) == Some(true);
        modules.push(ModuleSpec {
            rel,
            name,
            doc,
            changed,
            satisfies,
            planned,
        });
    }

    // 3. Sources.
    let adapter = crate::lang::adapter_for(&layout.spec.project.language);
    for m in &mut modules {
        if m.planned {
            continue;
        }
        let as_value: toml::Value =
            toml::from_str(&m.doc.to_string()).unwrap_or(toml::Value::Boolean(false));
        let source = adapter.as_deref().and_then(|a| {
            resolve_source_path(
                &m.name,
                source_file_override(&as_value),
                a,
                root,
                &layout.spec.paths.source,
            )
        });
        match source {
            None => {
                if opts.plan_unresolved {
                    m.doc["planned"] = value(true);
                    m.planned = true;
                    m.changed = true;
                    report.planned.push(m.name.clone());
                } else {
                    report.unresolved.push(m.name.clone());
                }
            }
            Some(src) => {
                let rel = rel_to(root, &src);
                if let Some(a) = adapter.as_deref() {
                    if crate::test_discovery::holds_only_tests(a, root, &rel)? {
                        report.tests_only.push(m.name.clone());
                    }
                }
            }
        }
    }
    for m in &modules {
        if m.changed {
            ov.write(&m.rel, m.doc.to_string().into_bytes());
        }
    }

    // 4. Requirements: `verified_by`, then `test_pending`.
    let req_dir = rel_to(
        root,
        &EntryStore::at(&layout.specs_dir, EntryLayout::PerEntry)
            .expected_file(EntryKind::Requirement),
    );
    let mut flagged = 0;
    for rel in ov.toml_files(&req_dir) {
        let text = utf8(ov.read(&rel).unwrap_or_default(), &rel)?;
        let mut doc: DocumentMut = text
            .parse()
            .with_context(|| format!("parsing {}", rel.display()))?;
        let Some(req) = doc
            .get_mut(EntryKind::Requirement.table())
            .and_then(Item::as_array_of_tables_mut)
            .and_then(|a| a.get_mut(0))
        else {
            continue;
        };
        let Some(id) = req.get("id").and_then(Item::as_str).map(str::to_string) else {
            continue;
        };
        let mut changed = false;
        let mut names = string_list(req.get("verified_by"));
        for n in verified.get(&id).into_iter().flatten() {
            if !names.contains(n) {
                names.push(n.clone());
                changed = true;
            }
        }
        if changed {
            let mut arr = Array::new();
            for n in &names {
                arr.push(n.as_str());
            }
            req["verified_by"] = value(arr);
        }
        let nature = req.get("nature").and_then(Item::as_str).unwrap_or("module");
        let satisfiers: Vec<&ModuleSpec> = modules
            .iter()
            .filter(|m| m.satisfies.contains(&id))
            .collect();
        let planned = req.get("planned").and_then(Item::as_bool) == Some(true)
            || (!satisfiers.is_empty() && satisfiers.iter().all(|m| m.planned));
        if matches!(nature, "module" | "test")
            && !planned
            && names.is_empty()
            && req.get("test_pending").is_none()
        {
            req["test_pending"] = value(true);
            changed = true;
            flagged += 1;
        }
        if changed {
            ov.write(&rel, doc.to_string().into_bytes());
        }
    }
    if !verified.is_empty() {
        report.lines.push(format!(
            "{verb}set `verified_by` on {} requirement(s)",
            verified.len()
        ));
    }
    if flagged > 0 {
        report.lines.push(format!(
            "{verb}set `test_pending = true` on {flagged} requirement(s): built (or not marked planned), no test named"
        ));
    }

    // 5. Profile files.
    let profile_dir = plugin_root.join("profiles").join(&profile);
    if !profile_dir.is_dir() {
        bail!(
            "[MIGRATE-002] profile `{profile}` not found in the plugin tree at {}",
            profile_dir.display()
        );
    }
    let inputs = BootstrapInputs {
        profile: profile.clone(),
        language: layout.spec.project.language.clone(),
        project_name: layout.spec.project.name.clone(),
        target_dir: root.clone(),
    };
    let expected = bootstrap::profile_expected_bytes(&profile_dir, &inputs)?;
    let recorded = lineage.profile_files.clone();
    let mut records: BTreeMap<String, String> = BTreeMap::new();
    let mut refused: Vec<String> = Vec::new();
    let spec_rel = PathBuf::from("spec.toml");
    let domain_rel = specdev_rel.join("domain.toml");
    let profile_rel = specdev_rel.join("profile.yaml");
    let reductions_rel = specdev_rel.join("reductions");
    let schemas_rel = specdev_rel.join("schemas");
    let mut handled: BTreeSet<PathBuf> = BTreeSet::new();
    // `architecture.toml`: the removed keys go (reported); a file with no key
    // left is deleted when the profile's 2.0 template has no key either (the
    // template is then gone, so it is not written back).
    let arch_rel = rel_to(root, &layout.architecture_toml_path);
    let arch_key = slash(&arch_rel);
    let mut deleted: BTreeSet<String> = BTreeSet::new();
    if let Some(ours) = ov.read(&arch_rel) {
        handled.insert(arch_rel.clone());
        let text = utf8(ours.clone(), &arch_rel)?;
        let (stripped, cut, keys_left) = crate::field_cuts::strip_architecture(&text)?;
        report_removed(&arch_rel, cut, &mut report.lines);
        let clean = recorded
            .get(&arch_key)
            .is_some_and(|r| bootstrap::profile_file_matches(&arch_key, &ours, r).unwrap_or(false));
        match expected.get(&arch_key) {
            Some(theirs) if clean => {
                if &ours != theirs {
                    ov.write(&arch_rel, theirs.clone());
                    records.insert(
                        arch_key.clone(),
                        bootstrap::profile_file_hash(&arch_key, theirs)?,
                    );
                    report.lines.push(format!("{verb}refresh {arch_key} to the plugin's {TARGET_VERSION} copy (unchanged since bootstrap)"));
                }
            }
            None if !keys_left => {
                ov.deletes.insert(arch_rel.clone());
                deleted.insert(arch_key.clone());
                let left: Vec<String> = stripped
                    .lines()
                    .filter(|l| l.trim_start().starts_with('#'))
                    .map(str::to_string)
                    .collect();
                let mut line =
                    format!("{verb}delete {arch_key} (no key left; the profile ships no template)");
                for c in left {
                    line.push_str(&format!("\n      {c}  [prose]"));
                }
                report.lines.push(line);
            }
            theirs => {
                if stripped != text {
                    ov.write(&arch_rel, stripped.into_bytes());
                } else if theirs.is_some_and(|b| b != &ours) {
                    report.lines.push(format!("{verb}left {arch_key} as is (it differs from the plugin's copy; `drift` shows it)"));
                }
            }
        }
    }
    for (key, theirs) in &expected {
        if key == &arch_key {
            continue;
        }
        let rel = PathBuf::from(key);
        handled.insert(rel.clone());
        let mechanical = rel == spec_rel
            || rel == domain_rel
            || rel == profile_rel
            || rel.starts_with(&reductions_rel);
        let Some(ours) = ov.read(&rel) else {
            if recorded.contains_key(key) {
                // Recorded but missing: the user (or a migration) deleted it.
                // Writing it back would undo that; `drift` reports it.
                report.lines.push(format!(
                    "{verb}leave {key} deleted (recorded, but not in the project)"
                ));
                continue;
            }
            ov.write(&rel, theirs.clone());
            records.insert(key.clone(), bootstrap::profile_file_hash(key, theirs)?);
            report
                .lines
                .push(format!("{verb}write {key} (new in the plugin)"));
            continue;
        };
        let ours_hash = bootstrap::profile_file_hash(key, &ours)?;
        let theirs_hash = bootstrap::profile_file_hash(key, theirs)?;
        let clean = match recorded.get(key) {
            Some(r) => bootstrap::profile_file_matches(key, &ours, r)?,
            None => false,
        };
        if clean && recorded.get(key) != Some(&ours_hash) {
            // Proven unedited in another line-ending form: re-record it in
            // the LF form (#219).
            records.insert(key.clone(), ours_hash.clone());
        }
        if clean {
            if rel == domain_rel {
                // Whole-file refresh; still say what changes for the rules.
                let (_, changes) = domain_2_0(&utf8(ours.clone(), &rel)?)?;
                for c in changes {
                    report.lines.push(format!("{verb}{c} in {key}"));
                }
            }
            if ours_hash != theirs_hash
                || (!bootstrap::CO_OWNED_AGGREGATORS.contains(&key.as_str()) && &ours != theirs)
            {
                let new = take_plugin_bytes(key, &ours, theirs)?;
                ov.write(&rel, new);
                records.insert(key.clone(), theirs_hash);
                report.lines.push(format!("{verb}refresh {key} to the plugin's {TARGET_VERSION} copy (unchanged since bootstrap)"));
            }
            continue;
        }
        if mechanical {
            let text = utf8(ours.clone(), &rel)?;
            let new = if rel == spec_rel {
                let (new, removed) = spec_2_0(&text)?;
                report_removed(&rel, removed, &mut report.lines);
                new
            } else if rel == domain_rel {
                let (new, changes) = domain_2_0(&text)?;
                for c in changes {
                    report.lines.push(format!("{verb}{c} in {key}"));
                }
                new
            } else if rel == profile_rel {
                let (new, removed) = profile_contract_2(&text);
                for r in removed {
                    report.lines.push(format!("{verb}remove from {key}:\n{r}"));
                }
                new
            } else {
                let (new, kinds) = crate::field_cuts::drop_reduction_kinds(
                    &entries::rewrite_reduction_paths(&text),
                );
                for k in kinds {
                    report.lines.push(format!(
                        "{verb}remove the `{k}` entry from {key} (it read a removed field)"
                    ));
                }
                new
            };
            if new.as_bytes() != ours.as_slice() {
                if rel == profile_rel {
                    report.lines.push(format!(
                        "{verb}remove the manifest-emitter sections and gate `scope` from {key}; contract_version 2"
                    ));
                } else if rel != spec_rel && rel != domain_rel {
                    report
                        .lines
                        .push(format!("{verb}point {key} at the per-entry files"));
                }
                if bootstrap::profile_file_hash(key, new.as_bytes())? == theirs_hash {
                    records.insert(key.clone(), theirs_hash);
                }
                ov.write(&rel, new.into_bytes());
            }
            continue;
        }
        if rel.starts_with(&schemas_rel) && ours_hash != theirs_hash {
            if recorded.contains_key(key) {
                refused.push(key.clone());
            } else {
                let new = take_plugin_bytes(key, &ours, theirs)?;
                // What the replacement removes: the lines of the project's copy
                // the plugin's copy does not have.
                let new_text = String::from_utf8_lossy(&new).into_owned();
                let keep: std::collections::HashSet<&str> =
                    new_text.lines().map(str::trim_end).collect();
                let ours_text = String::from_utf8_lossy(&ours).into_owned();
                let lost: Vec<String> = ours_text
                    .lines()
                    .map(str::trim_end)
                    .filter(|l| !l.is_empty() && !keep.contains(l))
                    .map(|l| format!("      - {l}"))
                    .collect();
                report.replaced_unrecorded.push(if lost.is_empty() {
                    key.clone()
                } else {
                    format!(
                        "{key} — lines the replacement removes:\n{}",
                        lost.join("\n")
                    )
                });
                ov.write(&rel, new);
                records.insert(key.clone(), theirs_hash.clone());
                report.lines.push(format!(
                    "{verb}replace {key} with the plugin's {TARGET_VERSION} copy (no bootstrap record; see the warning below)"
                ));
            }
        }
        if !rel.starts_with(&schemas_rel) && ours_hash != theirs_hash {
            // Neither a precise edit nor a schema: left as it is, and said so.
            report.lines.push(format!(
                "{verb}left {key} as is (it differs from the plugin's copy; `drift` shows it)"
            ));
        }
    }
    // The inlined language tier is a schema with no record (bootstrap never
    // recorded it): case c — replaced, listed with what it loses.
    let langs_rel = schemas_rel.join("inlined").join("languages");
    if let Ok(rd) = std::fs::read_dir(root.join(&langs_rel)) {
        for e in rd.flatten() {
            let lang = e.file_name().to_string_lossy().into_owned();
            let rel = langs_rel.join(&lang).join("items.schema.yaml");
            let Some(ours) = ov.read(&rel) else { continue };
            let Ok(theirs) = std::fs::read(
                plugin_root
                    .join("languages")
                    .join(&lang)
                    .join("items.schema.yaml"),
            ) else {
                continue;
            };
            if ours == theirs {
                continue;
            }
            let theirs_text = String::from_utf8_lossy(&theirs).into_owned();
            let keep: std::collections::HashSet<&str> =
                theirs_text.lines().map(str::trim_end).collect();
            let ours_text = String::from_utf8_lossy(&ours).into_owned();
            let lost: Vec<String> = ours_text
                .lines()
                .map(str::trim_end)
                .filter(|l| !l.is_empty() && !keep.contains(l))
                .map(|l| format!("      - {l}"))
                .collect();
            let key = slash(&rel);
            report.lines.push(format!(
                "{verb}replace {key} with the plugin's {TARGET_VERSION} copy (no bootstrap record; see the warning below)"
            ));
            report.replaced_unrecorded.push(if lost.is_empty() {
                key.clone()
            } else {
                format!(
                    "{key} — lines not in the plugin's copy:\n{}",
                    lost.join("\n")
                )
            });
            ov.write(&rel, theirs);
        }
    }

    // Reductions the profile does not own (inlined units) still name files.
    for rel in walk_files(&root.join(&reductions_rel))
        .into_iter()
        .map(|p| rel_to(root, &p))
    {
        if handled.contains(&rel) {
            continue;
        }
        let text = utf8(ov.read(&rel).unwrap_or_default(), &rel)?;
        let new = entries::rewrite_reduction_paths(&text);
        if new != text {
            report.lines.push(format!(
                "{verb}point {} at the per-entry files",
                slash(&rel)
            ));
            ov.write(&rel, new.into_bytes());
        }
    }
    // A domain.toml written above from the plugin needs no edits; one the
    // project changed got them. A missing one has nothing to rename.
    if !refused.is_empty() {
        bail!(
            "[MIGRATE-014] these schemas changed since bootstrap and the plugin's {TARGET_VERSION} copy differs; \
             nothing was changed. Merge each with the plugin's copy (profiles/{profile}/schemas/), or restore \
             the bootstrap copy, then run migrate again:\n  {}",
            refused.join("\n  ")
        );
    }

    // An unrecorded tree gets its first baseline: the plugin's 2.0 hash for
    // every profile file present. A file equal to the plugin's copy reads
    // clean; one that differs reads as locally edited — which a pre-2.0 edit
    // may be — so `--refresh` refuses it instead of overwriting it.
    if recorded.is_empty() {
        for (key, theirs) in &expected {
            if records.contains_key(key) || ov.read(Path::new(key)).is_none() {
                continue;
            }
            records.insert(key.clone(), bootstrap::profile_file_hash(key, theirs)?);
        }
        report.lines.push(format!(
            "{verb}record a first [profile_files] baseline (the tree had none)"
        ));
    }

    // 6. Generation state.
    let gen_lineage = specdev_rel.join("gen-lineage.toml");
    if ov.read(&gen_lineage).is_some() {
        ov.deletes.insert(gen_lineage.clone());
        report
            .lines
            .push(format!("{verb}remove {}", slash(&gen_lineage)));
    }
    let codegen = specdev_rel.join("codegen");
    if root.join(&codegen).is_dir() {
        ov.delete_dirs.insert(codegen.clone());
        report
            .lines
            .push(format!("{verb}remove {}/", slash(&codegen)));
    }

    // 6b. Unit snapshots lose their `codegen/` files with the inlined
    // `codegen/`; each unit is re-recorded from the snapshot that remains.
    for (kind, names) in [
        (
            crate::upstream::UnitKind::Block,
            lineage.blocks.keys().cloned().collect::<Vec<_>>(),
        ),
        (
            crate::upstream::UnitKind::Bridge,
            lineage.bridges.keys().cloned().collect::<Vec<_>>(),
        ),
    ] {
        for name in names {
            let snap = crate::upstream::read_upstream_unit(&layout.specdev_root, kind, &name)?;
            if !snap.keys().any(|p| p.starts_with("codegen")) {
                continue;
            }
            let dir = rel_to(
                root,
                &crate::upstream::upstream_dir(&layout.specdev_root, kind, &name),
            )
            .join("codegen");
            ov.delete_dirs.insert(dir.clone());
            let kept: BTreeMap<PathBuf, Vec<u8>> = snap
                .into_iter()
                .filter(|(p, _)| !p.starts_with("codegen"))
                .collect();
            let hash = crate::inline::snapshot_rollup(kind, &name, &kept);
            let entry = match kind {
                crate::upstream::UnitKind::Block => lineage.blocks.get_mut(&name),
                crate::upstream::UnitKind::Bridge => lineage.bridges.get_mut(&name),
            };
            if let Some(e) = entry {
                e.content_hash = hash;
            }
            report.lines.push(format!(
                "{verb}remove {}/ (the unit's codegen files) and re-record {} `{name}` from its remaining snapshot",
                slash(&dir),
                match kind {
                    crate::upstream::UnitKind::Block => "block",
                    crate::upstream::UnitKind::Bridge => "bridge",
                }
            ));
        }
    }

    // 7. Lineage.
    let moved_combined: Vec<String> = entries::combined_files_rel(&specs_rel)
        .iter()
        .map(|p| slash(p))
        .collect();
    let before = lineage.profile_files.len();
    lineage.profile_files.retain(|k, _| {
        let p = PathBuf::from(k);
        let gone = moved_combined.contains(k) || p.starts_with(&codegen) || deleted.contains(k);
        !(gone && !expected.contains_key(k))
    });
    let dropped = before - lineage.profile_files.len();
    if dropped > 0 {
        report.lines.push(format!(
            "{verb}drop {dropped} record(s) of moved or removed files from [profile_files]"
        ));
    }
    lineage.profile_files.extend(records);
    if let Some(meta) = lineage.meta.as_mut() {
        if let Some(v) = meta.plugin_version.take() {
            meta.bootstrapped_with.get_or_insert(v);
            report.lines.push(format!(
                "{verb}rename lineage.toml [meta].plugin_version to bootstrapped_with"
            ));
        }
    }

    // 8. spec.toml, last.
    if !ov.writes.contains_key(&spec_rel) {
        let text = utf8(ov.read(&spec_rel).unwrap_or_default(), &spec_rel)?;
        let (new, removed) = spec_2_0(&text)?;
        report_removed(&spec_rel, removed, &mut report.lines);
        ov.write(&spec_rel, new.into_bytes());
    }
    report.lines.push(format!(
        "{verb}set spec_version {} -> {TARGET_VERSION} and [project].max_parallel_agents (written last)",
        layout.spec.spec_version
    ));

    // Every planned `spec.toml` and `profile.yaml` must load before any byte
    // is written: an edit that broke one would leave a tree that refuses
    // every command.
    if let Some(bytes) = ov.writes.get(&spec_rel) {
        crate::config::SpecConfig::parse(&root.join(&spec_rel), &utf8(bytes.clone(), &spec_rel)?)
            .map_err(|e| {
            anyhow!(
                "[MIGRATE-016] the planned spec.toml does not load ({e:#}); nothing was changed"
            )
        })?;
    }
    if let Some(bytes) = ov.writes.get(&profile_rel) {
        crate::manifest::profile::Profile::check_text(
            &utf8(bytes.clone(), &profile_rel)?,
            &root.join(&profile_rel),
        )
        .map_err(|e| {
            anyhow!(
                "[MIGRATE-016] the planned {} does not load ({e:#}); nothing was changed",
                slash(&profile_rel)
            )
        })?;
    }

    report.removed_values = removed_count.get();
    Ok(Plan {
        ov,
        lineage: Some(lineage),
        specdev_root: layout.specdev_root.clone(),
        report,
    })
}

/// The plugin's bytes for `key`; a co-owned aggregator keeps the unit
/// `$ref` entries the project's copy holds beyond the plugin's own.
fn take_plugin_bytes(key: &str, ours: &[u8], theirs: &[u8]) -> Result<Vec<u8>> {
    if !bootstrap::CO_OWNED_AGGREGATORS.contains(&key) {
        return Ok(theirs.to_vec());
    }
    let refs = |bytes: &[u8]| -> Result<Vec<serde_yaml::Value>> {
        let v: serde_yaml::Value =
            serde_yaml::from_slice(bytes).with_context(|| format!("parsing {key}"))?;
        Ok(v.get("allOf")
            .and_then(|a| a.as_sequence())
            .cloned()
            .unwrap_or_default())
    };
    let ours_refs = refs(ours)?;
    let theirs_refs = refs(theirs)?;
    let extra: Vec<serde_yaml::Value> = ours_refs
        .into_iter()
        .filter(|r| !theirs_refs.contains(r))
        .collect();
    if extra.is_empty() {
        return Ok(theirs.to_vec());
    }
    let mut v: serde_yaml::Value =
        serde_yaml::from_slice(theirs).with_context(|| format!("parsing {key}"))?;
    let map = v
        .as_mapping_mut()
        .ok_or_else(|| anyhow!("{key}: root is not a mapping"))?;
    let seq = map
        .entry(serde_yaml::Value::from("allOf"))
        .or_insert(serde_yaml::Value::Sequence(Vec::new()))
        .as_sequence_mut()
        .ok_or_else(|| anyhow!("{key}: `allOf` is not a sequence"))?;
    seq.extend(extra);
    Ok(serde_yaml::to_string(&v)?.into_bytes())
}

/// Remove every `test = true` item; return `(name, verifies)` for each.
fn take_test_items(doc: &mut DocumentMut) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut now_empty = false;
    if let Some(items) = doc.get_mut("items").and_then(Item::as_array_of_tables_mut) {
        let mut keep = toml_edit::ArrayOfTables::new();
        for t in items.iter() {
            if t.get("test").and_then(Item::as_bool) == Some(true) {
                let name = t
                    .get("name")
                    .and_then(Item::as_str)
                    .unwrap_or("")
                    .to_string();
                out.push((name, string_list(t.get("verifies"))));
            } else {
                keep.push(t.clone());
            }
        }
        if !out.is_empty() {
            now_empty = keep.is_empty();
            *items = keep;
        }
    } else if let Some(arr) = doc.get_mut("items").and_then(Item::as_array_mut) {
        let before = arr.len();
        let mut i = 0;
        while i < arr.len() {
            let is_test = arr
                .get(i)
                .and_then(Value::as_inline_table)
                .and_then(|t| t.get("test"))
                .and_then(Value::as_bool)
                == Some(true);
            if is_test {
                let t = arr
                    .get(i)
                    .and_then(Value::as_inline_table)
                    .cloned()
                    .unwrap_or_default();
                let name = t
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let verifies = t
                    .get("verifies")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                out.push((name, verifies));
                arr.remove(i);
            } else {
                i += 1;
            }
        }
        now_empty = before > 0 && arr.is_empty();
    }
    if now_empty {
        doc.remove("items");
    }
    out
}

/// An inlined `profile.yaml` at contract 2, edited line by line so every
/// comment and every other byte stays: the retired top-level sections go
/// (a section ends at the next line in column 0), and so do each gate's
/// `scope` line and the prompt-only `context.module_fields_for_prompt`.
fn profile_contract_2(text: &str) -> (String, Vec<String>) {
    let retired = crate::manifest::profile::RETIRED_SECTIONS;
    let top_key = |l: &str| -> Option<String> {
        if l.starts_with(' ') || l.starts_with('#') || l.trim().is_empty() {
            return None;
        }
        l.split(':').next().map(|k| k.trim().to_string())
    };
    let mut out = String::with_capacity(text.len());
    let mut section = String::new();
    let mut skipping = false;
    let mut skip_indent: Option<usize> = None;
    let mut has_version = false;
    let mut removed: Vec<String> = Vec::new();
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    for (i, line) in lines.iter().copied().enumerate() {
        // A column-0 comment ends a retired section only when the next content
        // line is at column 0 too: then it belongs to what follows.
        if skipping && line.starts_with('#') {
            let next = lines[i + 1..]
                .iter()
                .find(|l| !l.trim().is_empty() && !l.starts_with('#'));
            if next.is_none_or(|l| !l.starts_with(' ')) {
                skipping = false;
            }
        }
        if let Some(k) = top_key(line) {
            section = k.clone();
            skipping = retired.contains(&k.as_str());
            skip_indent = None;
            if skipping {
                removed.push(String::new());
            }
            if k == "contract_version" {
                has_version = true;
                out.push_str("contract_version: 2\n");
                continue;
            }
        }
        if skipping {
            if let Some(last) = removed.last_mut() {
                last.push_str("    ");
                last.push_str(line);
            }
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if let Some(n) = skip_indent {
            if indent > n && !line.trim().is_empty() {
                if let Some(last) = removed.last_mut() {
                    last.push_str("    ");
                    last.push_str(line);
                }
                continue;
            }
            skip_indent = None;
        }
        let trimmed = line.trim();
        let scope_value = trimmed
            .strip_prefix("scope:")
            .map(|v| v.split('#').next().unwrap_or("").trim());
        if section == "gates" && matches!(scope_value, Some("module") | Some("batch")) {
            removed.push(format!("    {line}"));
            continue;
        }
        if section == "context" && trimmed.starts_with("module_fields_for_prompt:") {
            skip_indent = Some(indent);
            removed.push(format!("    {line}"));
            continue;
        }
        out.push_str(line);
    }
    if !has_version {
        out = format!("contract_version: 2\n{out}");
    }
    let removed = removed
        .into_iter()
        .map(|r| r.trim_end().to_string())
        .filter(|r| !r.is_empty())
        .collect();
    (out, removed)
}

/// `spec.toml` in the target format: the version, and the parallel-agent
/// limit when absent. Every other byte stays.
fn spec_2_0(text: &str) -> Result<(String, Vec<crate::field_cuts::Removed>)> {
    let mut doc: DocumentMut = text.parse().context("parsing spec.toml")?;
    doc["spec_version"] = value(TARGET_VERSION);
    let mut removed = Vec::new();
    if let Some(project) = doc.get_mut("project").and_then(Item::as_table_mut) {
        for k in crate::config::RETIRED_PROJECT_KEYS {
            if let Some(v) = project.remove(k) {
                removed.push(spec_removed("project", k, &v));
            }
        }
        if !project.contains_key("max_parallel_agents") {
            project.insert("max_parallel_agents", value(1));
        }
    }
    if let Some(ws) = doc.get_mut("workspace").and_then(Item::as_table_mut) {
        for k in crate::config::RETIRED_WORKSPACE_KEYS {
            if let Some(v) = ws.remove(k) {
                removed.push(spec_removed("workspace", k, &v));
            }
        }
    }
    Ok((doc.to_string(), removed))
}

fn spec_removed(table: &str, key: &str, item: &Item) -> crate::field_cuts::Removed {
    let value = item
        .as_value()
        .map(|v| v.to_string().trim().to_string())
        .unwrap_or_default();
    let raw = toml::from_str::<toml::Table>(&format!("x = {value}"))
        .ok()
        .and_then(|t| t.get("x").cloned());
    let comments = item
        .as_value()
        .map(|v| {
            [v.decor().prefix(), v.decor().suffix()]
                .into_iter()
                .flatten()
                .filter_map(|r| r.as_str())
                .flat_map(|s| {
                    s.lines()
                        .map(str::trim)
                        .filter(|l| l.starts_with('#'))
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .collect()
        })
        .unwrap_or_default();
    crate::field_cuts::Removed {
        key: format!("[{table}].{key}"),
        value,
        comments,
        raw,
    }
}

/// `domain.toml` in the target format, and one line per change.
fn domain_2_0(text: &str) -> Result<(String, Vec<String>)> {
    let mut changes = Vec::new();
    let mut doc: DocumentMut = text.parse().context("parsing domain.toml")?;
    // Rename: the rules-table key keeps its place, and each graph check's
    // `rule` value.
    for (old, new) in RENAMED_RULES {
        let mut renamed = false;
        if let Some(rules) = doc
            .get_mut("validation")
            .and_then(|v| v.get_mut("rules"))
            .and_then(Item::as_table_like_mut)
        {
            if let Some(v) = rules.remove(old) {
                rules.insert(new, v);
                renamed = true;
            }
        }
        if let Some(checks) = graph_checks(&mut doc) {
            for t in checks.iter_mut() {
                if t.get("rule").and_then(Item::as_str) == Some(old) {
                    let decor = t
                        .get("rule")
                        .and_then(Item::as_value)
                        .map(|v| v.decor().clone());
                    let mut v = Value::from(*new);
                    if let Some(d) = decor {
                        *v.decor_mut() = d;
                    }
                    t["rule"] = Item::Value(v);
                    renamed = true;
                }
            }
        }
        if renamed {
            changes.push(format!("rename rule `{old}` to `{new}`"));
        }
    }
    // Severities.
    if let Some(rules) = doc
        .get_mut("validation")
        .and_then(|v| v.get_mut("rules"))
        .and_then(Item::as_table_like_mut)
    {
        for rule in ERROR_RULES {
            let Some(item) = rules.get_mut(rule) else {
                continue;
            };
            let old = item.as_str().unwrap_or("").to_string();
            if old != "error" {
                let decor = item.as_value().map(|v| v.decor().clone());
                let mut v = Value::from("error");
                if let Some(d) = decor {
                    *v.decor_mut() = d;
                }
                *item = Item::Value(v);
                changes.push(format!("set `{rule}` from \"{old}\" to \"error\""));
            }
        }
        for rule in DROPPED_RULES {
            if rules.remove(rule).is_some() {
                changes.push(format!("remove the `{rule}` severity"));
            }
        }
    }
    if let Some(checks) = graph_checks(&mut doc) {
        let before = checks.len();
        checks.retain(|t| {
            let rule = t.get("rule").and_then(Item::as_str).unwrap_or("");
            let over_removed_kind = ["scope", "entity_scope", "relation_scope"].iter().any(|k| {
                t.get(k)
                    .and_then(Item::as_str)
                    .is_some_and(|s| crate::field_cuts::REMOVED_KINDS.contains(&s))
            });
            !DROPPED_RULES.contains(&rule) && !over_removed_kind
        });
        if checks.len() != before {
            changes.push(format!(
                "remove {} graph check(s) over removed fields",
                before - checks.len()
            ));
        }
    }
    Ok((doc.to_string(), changes))
}

fn graph_checks(doc: &mut DocumentMut) -> Option<&mut toml_edit::ArrayOfTables> {
    doc.get_mut("project")
        .and_then(|p| p.get_mut("graph_checks"))
        .and_then(Item::as_array_of_tables_mut)
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk_files(&p));
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}
