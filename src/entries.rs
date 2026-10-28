//! Where a project keeps its decisions, requirements, sources and open
//! questions.
//!
//! This module is the one place in `src/` that knows those files. Every
//! reader goes through [`EntryStore`]: the prompt's decision and
//! open-question context, the codegen gate, `validate`'s schema pass and
//! summary counts, and the coverage report. A test
//! (`entry_file_names_live_in_one_place`) fails when another file in `src/`
//! names one of them, so the next layout change is a change here only.
//!
//! Two layouts, chosen by the major number of `spec_version`:
//!
//! - **`1.x`, [`EntryLayout::Combined`]:** three files under `specs/` —
//!   `decisions.toml` (`[[decision]]`), `requirements.toml` (`[[source]]`
//!   and `[[requirement]]`) and `open_questions.toml` (`[[open_question]]`).
//! - **`2.x`, [`EntryLayout::PerEntry`]:** one file per entry —
//!   `specs/decisions/<id>.toml`, `specs/requirements/<id>.toml`,
//!   `specs/sources/<id>.toml`, `specs/open_questions/<id>.toml` — each
//!   holding one table of its kind, so two branches that add different
//!   entries touch different files.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::config::SpecConfig;
use crate::layout::LayoutContext;

/// A kind of spec entry kept outside the module files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Decision,
    Requirement,
    Source,
    OpenQuestion,
}

impl EntryKind {
    /// The TOML array-of-tables key an entry of this kind sits under.
    pub fn table(self) -> &'static str {
        match self {
            EntryKind::Decision => "decision",
            EntryKind::Requirement => "requirement",
            EntryKind::Source => "source",
            EntryKind::OpenQuestion => "open_question",
        }
    }

    /// Every kind.
    pub const ALL: [EntryKind; 4] = [
        EntryKind::Decision,
        EntryKind::Requirement,
        EntryKind::Source,
        EntryKind::OpenQuestion,
    ];

    /// The `1.x` file under `specs/` that holds this kind. Requirements and
    /// sources share one.
    fn combined_file(self) -> &'static str {
        match self {
            EntryKind::Decision => "decisions.toml",
            EntryKind::Requirement | EntryKind::Source => "requirements.toml",
            EntryKind::OpenQuestion => "open_questions.toml",
        }
    }

    /// The `2.x` directory under `specs/` that holds one file per entry.
    fn entry_dir(self) -> &'static str {
        match self {
            EntryKind::Decision => "decisions",
            EntryKind::Requirement => "requirements",
            EntryKind::Source => "sources",
            EntryKind::OpenQuestion => "open_questions",
        }
    }
}

/// How a project keeps its entries on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryLayout {
    /// `spec_version` `1.x`: one file per kind.
    Combined,
    /// `spec_version` `2.x`: one file per entry.
    PerEntry,
}

impl EntryLayout {
    /// The layout a `spec.toml` names.
    pub fn of(spec: &SpecConfig) -> Self {
        if spec.format_major() >= 2 {
            EntryLayout::PerEntry
        } else {
            EntryLayout::Combined
        }
    }
}

/// The first line of a tombstone: what `migrate` leaves at a combined
/// file's path after moving its entries. Not valid TOML, so an analyzer that
/// predates the per-entry layout fails loudly on it instead of reading it
/// as empty.
pub const TOMBSTONE_PREFIX: &str = "MOVED to ";

/// The second (last) line of every tombstone.
pub const TOMBSTONE_SECOND_LINE: &str =
    "This file is not TOML on purpose: an analyzer that reads the old layout stops here.";

/// The first line, by its whole shape: `MOVED to <dir>[ and <dir>]… by
/// specdev-analyzer migrate — upgrade specdev-analyzer to <x.y.z> or later`,
/// each `<dir>` a `specs/<name>/` path.
fn is_tombstone_first_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix(TOMBSTONE_PREFIX) else {
        return false;
    };
    let Some((dirs, tail)) =
        rest.split_once(" by specdev-analyzer migrate — upgrade specdev-analyzer to ")
    else {
        return false;
    };
    let Some(version) = tail.strip_suffix(" or later") else {
        return false;
    };
    let version_ok = version.split('.').count() == 3
        && version
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let dirs_ok = dirs.split(" and ").all(|d| {
        d.strip_prefix("specs/")
            .and_then(|x| x.strip_suffix('/'))
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
    });
    version_ok && dirs_ok
}

/// Whether `text` is exactly a tombstone `migrate` wrote (line endings
/// ignored): its two lines and nothing else. A tombstone with anything
/// appended — entries from a merge — is legacy content, not a tombstone.
pub fn is_tombstone(text: &str) -> bool {
    let lf = text.replace("\r\n", "\n");
    let mut lines = lf.trim_end_matches('\n').split('\n');
    match (lines.next(), lines.next(), lines.next()) {
        (Some(a), Some(b), None) => is_tombstone_first_line(a) && b == TOMBSTONE_SECOND_LINE,
        _ => false,
    }
}

/// `text` without a leading tombstone, when it starts with one: what a merge
/// appended under it. `None` when it does not start with a tombstone.
fn after_tombstone(text: &str) -> Option<String> {
    let lf = text.replace("\r\n", "\n");
    let mut it = lf.splitn(3, '\n');
    let (a, b) = (it.next()?, it.next()?);
    (is_tombstone_first_line(a) && b == TOMBSTONE_SECOND_LINE)
        .then(|| it.next().unwrap_or("").to_string())
}

/// One entry: the table it was read from and the file that holds it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub file: PathBuf,
    pub value: toml::Value,
}

impl Entry {
    /// The entry's `id`, when it has a string one.
    pub fn id(&self) -> Option<&str> {
        self.value.get("id").and_then(toml::Value::as_str)
    }
}

/// The entry files of one project's (or one workspace root's) `specs/`
/// directory.
#[derive(Debug, Clone)]
pub struct EntryStore {
    specs_dir: PathBuf,
    layout: EntryLayout,
}

impl EntryStore {
    /// The project's own entries, in the layout its `spec_version` names.
    pub fn for_layout(layout: &LayoutContext) -> Self {
        Self::at(&layout.specs_dir, EntryLayout::of(&layout.spec))
    }

    /// The entries under a `specs/` directory, in a given layout.
    pub fn at(specs_dir: &Path, layout: EntryLayout) -> Self {
        Self {
            specs_dir: specs_dir.to_path_buf(),
            layout,
        }
    }

    /// The entries of the project whose `spec.toml` is at `spec_toml`, in
    /// that project's own layout — how a member reads its workspace root.
    pub fn for_spec_toml(spec_toml: &Path) -> Result<Self> {
        let spec = SpecConfig::load(spec_toml)
            .with_context(|| format!("loading {}", spec_toml.display()))?;
        let root = spec_toml.parent().unwrap_or(Path::new("."));
        Ok(Self::at(
            &root.join(&spec.paths.specs),
            EntryLayout::of(&spec),
        ))
    }

    /// The store of the workspace root this project is a member of, found
    /// by the root's `[workspace].members[].path` listing this project.
    /// `None` for a standalone project.
    pub fn workspace_root_of(layout: &LayoutContext) -> Result<Option<Self>> {
        match crate::layout::find_workspace_root_spec(&layout.project_root)? {
            Some(root_spec) => Ok(Some(Self::for_spec_toml(&root_spec)?)),
            None => Ok(None),
        }
    }

    /// This store's layout.
    pub fn layout(&self) -> EntryLayout {
        self.layout
    }

    /// The files that exist and hold entries of `kind`, sorted by path.
    pub fn files(&self, kind: EntryKind) -> Vec<PathBuf> {
        match self.layout {
            EntryLayout::Combined => {
                let path = self.expected_file(kind);
                if path.is_file() {
                    vec![path]
                } else {
                    Vec::new()
                }
            }
            EntryLayout::PerEntry => {
                let dir = self.expected_file(kind);
                let mut files = toml_files_in(&dir);
                // Retired requirements stay loaded, from their own folder.
                if kind == EntryKind::Requirement {
                    files.extend(toml_files_in(&dir.join(RETIRED_DIR)));
                }
                files
            }
        }
    }

    /// Where entries of `kind` are kept when there are any: the combined
    /// file, or the per-entry directory. Named in the "none authored yet"
    /// finding, which needs a path when [`files`] is empty.
    ///
    /// [`files`]: EntryStore::files
    pub fn expected_file(&self, kind: EntryKind) -> PathBuf {
        match self.layout {
            EntryLayout::Combined => self.specs_dir.join(kind.combined_file()),
            EntryLayout::PerEntry => self.specs_dir.join(kind.entry_dir()),
        }
    }

    /// Whether any file holds entries of `kind`.
    pub fn has_any(&self, kind: EntryKind) -> bool {
        !self.files(kind).is_empty()
    }

    /// In the per-entry layout, the combined files that still hold entries,
    /// or do not parse and so might. A tombstone, and a file with no entry
    /// (comments only, as a bootstrap template is), are not legacy content.
    ///
    /// The per-entry reader does not read these files. Without this check a
    /// project whose `spec_version` says `2.x` while its entries sit in
    /// `1.x` files loses them silently, and an open question in them stops
    /// blocking code generation. Empty in the combined layout.
    pub fn legacy_content(&self) -> Vec<PathBuf> {
        if self.layout != EntryLayout::PerEntry {
            return Vec::new();
        }
        let mut out: Vec<PathBuf> = Vec::new();
        for kind in EntryKind::ALL {
            let path = self.specs_dir.join(kind.combined_file());
            if out.contains(&path) || !path.is_file() {
                continue;
            }
            let holds_entries = match std::fs::read_to_string(&path) {
                Ok(text) if is_tombstone(&text) => false,
                Ok(text) => match toml::from_str::<toml::Table>(&text) {
                    Ok(table) => EntryKind::ALL.iter().any(|k| {
                        k.combined_file() == kind.combined_file()
                            && table
                                .get(k.table())
                                .and_then(toml::Value::as_array)
                                .is_some_and(|a| !a.is_empty())
                    }),
                    Err(_) => true,
                },
                Err(_) => true,
            };
            if holds_entries {
                out.push(path);
            }
        }
        out
    }

    /// The `ENTRY-LEGACY-001` message for a combined file found by
    /// [`legacy_content`](EntryStore::legacy_content), without the code.
    pub fn legacy_message(path: &Path) -> String {
        format!(
            "{} still holds content, but this project's spec_version is 2.x, which \
             keeps one file per entry under specs/<kind>/. Its entries are not read. Run \
             `specdev-analyzer migrate` to move them; do not edit spec_version by hand.",
            path.display()
        )
    }

    /// Every entry of `kind`, in file order. An unreadable or unparseable
    /// file is an error, and so is legacy content for `kind` in the
    /// per-entry layout (`ENTRY-LEGACY-001`).
    pub fn load(&self, kind: EntryKind) -> Result<Vec<Entry>> {
        if self.layout == EntryLayout::PerEntry {
            let legacy = self.specs_dir.join(kind.combined_file());
            if self.legacy_content().contains(&legacy) {
                bail!("[ENTRY-LEGACY-001] {}", Self::legacy_message(&legacy));
            }
        }
        let mut out = Vec::new();
        for file in self.files(kind) {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("reading {}", file.display()))?;
            let parsed: toml::Value =
                toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
            push_entries(&mut out, &file, &parsed, kind);
        }
        Ok(out)
    }

    /// Every entry of `kind` from the files that read and parse; the others
    /// are skipped. For best-effort surfaces (summary counts, the test
    /// verification context) where another pass reports the broken file.
    pub fn load_lenient(&self, kind: EntryKind) -> Vec<Entry> {
        let mut out = Vec::new();
        for file in self.files(kind) {
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(parsed) = toml::from_str::<toml::Value>(&text) else {
                continue;
            };
            push_entries(&mut out, &file, &parsed, kind);
        }
        out
    }

    /// How a message names the place to edit entry `id`, in backticks:
    /// `` `specs/open_questions.toml` `` (`1.x`) or
    /// `` `specs/open_questions/OQ-002.toml` `` (`2.x`).
    pub fn location_hint(&self, kind: EntryKind, id: &str) -> String {
        match self.layout {
            EntryLayout::Combined => format!("`specs/{}`", kind.combined_file()),
            EntryLayout::PerEntry
                if kind == EntryKind::Requirement
                    && self
                        .expected_file(kind)
                        .join(RETIRED_DIR)
                        .join(format!("{id}.toml"))
                        .is_file() =>
            {
                format!("`specs/{}/{RETIRED_DIR}/{id}.toml`", kind.entry_dir())
            }
            EntryLayout::PerEntry => format!("`specs/{}/{id}.toml`", kind.entry_dir()),
        }
    }
}

/// The folder under the requirements directory that holds retired
/// requirements. The folder is the state: an agent that lists the
/// requirements directory does not see them.
pub const RETIRED_DIR: &str = "retired";

/// Whether `file` is a retired requirement's file: it sits directly in a
/// `retired/` folder under the requirements directory.
pub fn is_retired_file(file: &Path) -> bool {
    let Some(dir) = file.parent() else {
        return false;
    };
    dir.file_name().is_some_and(|n| n == RETIRED_DIR)
        && dir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == EntryKind::Requirement.entry_dir())
}

/// For a reduction `from` / `from_spec` glob that reads the per-entry
/// requirement files (`<specs>/requirements/*.toml`), the glob of the retired
/// ones beside it. The reducer reads both, so a retired requirement stays in
/// the graph with no change to the inlined reductions.
pub fn retired_glob_for(pattern: &str) -> Option<String> {
    let dir = EntryKind::Requirement.entry_dir();
    let tail = format!("{dir}/*.toml");
    let head = pattern.strip_suffix(&tail)?;
    (head.is_empty() || head.ends_with('/')).then(|| format!("{head}{dir}/{RETIRED_DIR}/*.toml"))
}

/// The `.toml` files directly in `dir`, sorted; none when it is absent.
fn toml_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("toml"))
        })
        .collect();
    files.sort();
    files
}

/// The first analyzer release that reads the per-entry layout; a
/// tombstone tells an older one to upgrade to it.
pub const FIRST_PER_ENTRY_ANALYZER: &str = "0.3.0";

/// What moving one project's combined files to the per-entry layout
/// writes. Paths are relative to the project's `specs/` directory.
#[derive(Debug, Default)]
pub struct SplitPlan {
    /// New per-entry files, and each directory's `README.md` holding the
    /// combined file's header comment.
    pub writes: Vec<(PathBuf, Vec<u8>)>,
    /// The tombstone that replaces each combined file.
    pub tombstones: Vec<(PathBuf, Vec<u8>)>,
    /// `(combined file, entries moved)`, for the report.
    pub moved: Vec<(PathBuf, usize)>,
}

/// Plan the move of every combined file under `specs_dir` into one file
/// per entry. Each entry keeps its bytes: the text from its `[[kind]]`
/// header to the next one, with a comment block directly above the next
/// header given to that entry. The plan is checked before it is returned —
/// every piece must parse to exactly the entry it was cut from — so text
/// that only looks like a header (inside a multi-line string) refuses
/// instead of splitting wrongly. A target file that exists with other
/// bytes refuses too: nothing is overwritten. A tombstone or an absent
/// file moves nothing.
pub fn plan_split(specs_dir: &Path) -> Result<SplitPlan> {
    let mut plan = SplitPlan::default();
    let mut conflicts: Vec<String> = Vec::new();
    let mut seen: Vec<&'static str> = Vec::new();
    for kind in EntryKind::ALL {
        let combined = kind.combined_file();
        if seen.contains(&combined) {
            continue;
        }
        seen.push(combined);
        let path = specs_dir.join(combined);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if is_tombstone(&text) {
            continue;
        }
        // Entries appended under a tombstone (a merge from an old branch) are
        // folded like any combined file; the tombstone is written again.
        let text = after_tombstone(&text).unwrap_or(text);
        let kinds: Vec<EntryKind> = EntryKind::ALL
            .into_iter()
            .filter(|k| k.combined_file() == combined)
            .collect();
        let parsed: toml::Table =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if let Some(other) = parsed
            .keys()
            .find(|k| !kinds.iter().any(|kind| kind.table() == k.as_str()))
        {
            bail!(
                "[MIGRATE-011] {} holds `{other}`, which is not an entry table; move it by hand first",
                path.display()
            );
        }
        let (header, pieces) = cut_entries(&text, &kinds);
        let mut count = 0;
        for (kind, piece) in &pieces {
            let one: toml::Table = toml::from_str(piece).with_context(|| {
                format!(
                    "[MIGRATE-012] {}: an entry does not parse on its own:\n{piece}",
                    path.display()
                )
            })?;
            let arr = one.get(kind.table()).and_then(toml::Value::as_array);
            let Some([entry]) = arr.map(Vec::as_slice) else {
                bail!(
                    "[MIGRATE-012] {}: a piece does not hold exactly one entry:\n{piece}",
                    path.display()
                );
            };
            let Some(id) = entry.get("id").and_then(toml::Value::as_str) else {
                bail!(
                    "[MIGRATE-012] {}: an entry has no `id`:\n{piece}",
                    path.display()
                );
            };
            let rel = PathBuf::from(kind.entry_dir()).join(format!("{id}.toml"));
            let abs = specs_dir.join(&rel);
            if let Ok(existing) = std::fs::read(&abs) {
                if existing != piece.as_bytes() {
                    conflicts.push(format!(
                        "{} (from {combined})\n    in the file:\n{}\n    in {combined}:\n{}",
                        abs.display(),
                        indent(&String::from_utf8_lossy(&existing)),
                        indent(piece)
                    ));
                }
                continue;
            }
            if plan.writes.iter().any(|(r, _)| r == &rel) {
                conflicts.push(format!(
                    "{} (two entries with id `{id}` in {combined})",
                    abs.display()
                ));
                continue;
            }
            plan.writes.push((rel, piece.clone().into_bytes()));
            count += 1;
        }
        // Every entry was cut out, and nothing else.
        let total: usize = kinds
            .iter()
            .map(|k| {
                parsed
                    .get(k.table())
                    .and_then(toml::Value::as_array)
                    .map_or(0, Vec::len)
            })
            .sum();
        if total != pieces.len() {
            bail!(
                "[MIGRATE-012] {}: {total} entries parse, but {} were cut out; split it by hand",
                path.display(),
                pieces.len()
            );
        }
        if !header.trim().is_empty() {
            let dir = kinds[0].entry_dir();
            let readme = PathBuf::from(dir).join("README.md");
            if let Ok(existing) = std::fs::read_to_string(specs_dir.join(&readme)) {
                // A fold (a merge brought a combined file back): keep its
                // header comments by appending them, unless already there.
                if !existing.contains(header.trim()) {
                    let body = format!(
                        "{}\n\nFolded later by `specdev-analyzer migrate` from `specs/{combined}`:\n\n```toml\n{}\n```\n",
                        existing.trim_end(),
                        header.trim_end()
                    );
                    plan.writes.push((readme, body.into_bytes()));
                }
            } else {
                let body = format!(
                    "# specs/{dir}/\n\nOne file per entry, `<id>.toml`. `specdev-analyzer migrate` moved the \
                     entries of `specs/{combined}` here; the header comment of that file follows.\n\n```toml\n{}\n```\n",
                    header.trim_end()
                );
                plan.writes.push((readme, body.into_bytes()));
            }
        }
        let dirs: Vec<String> = kinds
            .iter()
            .map(|k| format!("specs/{}/", k.entry_dir()))
            .collect();
        let tomb = format!(
            "{TOMBSTONE_PREFIX}{} by specdev-analyzer migrate — upgrade specdev-analyzer to {FIRST_PER_ENTRY_ANALYZER} or later\n\
             {TOMBSTONE_SECOND_LINE}\n",
            dirs.join(" and ")
        );
        plan.tombstones
            .push((PathBuf::from(combined), tomb.into_bytes()));
        plan.moved.push((PathBuf::from(combined), count));
    }
    if !conflicts.is_empty() {
        bail!(
            "[MIGRATE-010] these entry files exist with other bytes; nothing is overwritten. \
             Compare each with its entry in the combined file and keep one:\n  {}",
            conflicts.join("\n  ")
        );
    }
    Ok(plan)
}

fn indent(text: &str) -> String {
    text.lines()
        .map(|l| format!("      {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The header (text before the first entry) and each entry's text.
fn cut_entries(text: &str, kinds: &[EntryKind]) -> (String, Vec<(EntryKind, String)>) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let header_kind = |l: &str| {
        let t = l.trim();
        kinds
            .iter()
            .copied()
            .find(|k| t == format!("[[{}]]", k.table()))
    };
    let starts: Vec<(usize, EntryKind)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| header_kind(l).map(|k| (i, k)))
        .collect();
    // A comment block directly above a header (no blank line between)
    // belongs to that entry.
    let lead = |i: usize| {
        let mut s = i;
        while s > 0 && lines[s - 1].trim_start().starts_with('#') {
            s -= 1;
        }
        s
    };
    let first = starts.first().map(|(i, _)| lead(*i)).unwrap_or(lines.len());
    let header: String = lines[..first].concat();
    let mut pieces = Vec::new();
    for (n, (i, kind)) in starts.iter().enumerate() {
        let begin = lead(*i);
        let end = starts
            .get(n + 1)
            .map(|(j, _)| lead(*j))
            .unwrap_or(lines.len());
        let mut piece = lines[begin..end].concat();
        let trimmed = piece.trim_end().len();
        piece.truncate(trimmed);
        piece.push('\n');
        pieces.push((*kind, piece));
    }
    (header, pieces)
}

/// A reduction file's text with every `from:` / `from_spec:` that names a
/// combined file pointed at the per-entry glob. The one entry block whose
/// `for_each` reads `source[]` goes to the sources directory.
pub fn rewrite_reduction_paths(text: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    for (i, line) in lines.iter().enumerate() {
        let mut l = line.to_string();
        for kind in EntryKind::ALL {
            let old = format!("\"specs/{}\"", kind.combined_file());
            if !l.contains(&old) || kind == EntryKind::Source {
                continue;
            }
            let reads_sources =
                kind == EntryKind::Requirement && block_reads(&lines, i, EntryKind::Source);
            let target = if reads_sources {
                EntryKind::Source
            } else {
                kind
            };
            l = l.replace(&old, &format!("\"specs/{}/*.toml\"", target.entry_dir()));
        }
        out.push_str(&l);
    }
    out
}

/// Whether the YAML list item holding line `i` has `for_each: "<kind>[]"`.
fn block_reads(lines: &[&str], i: usize, kind: EntryKind) -> bool {
    let is_item = |l: &str| l.trim_start().starts_with("- ");
    let mut s = i;
    while s > 0 && !is_item(lines[s]) {
        s -= 1;
    }
    let mut e = i + 1;
    while e < lines.len() && !is_item(lines[e]) {
        e += 1;
    }
    let want = format!("for_each: \"{}[]\"", kind.table());
    lines[s..e].iter().any(|l| l.trim() == want)
}

/// The project-relative paths of the combined files, for the profile-file
/// record a move drops.
pub fn combined_files_rel(specs_rel: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for kind in EntryKind::ALL {
        let p = specs_rel.join(kind.combined_file());
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

fn push_entries(out: &mut Vec<Entry>, file: &Path, parsed: &toml::Value, kind: EntryKind) {
    if let Some(arr) = parsed.get(kind.table()).and_then(toml::Value::as_array) {
        out.extend(arr.iter().map(|value| Entry {
            file: file.to_path_buf(),
            value: value.clone(),
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "specdev-entries-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn requirements_and_sources_share_a_file_and_are_told_apart_by_table() {
        let dir = tmp("shared");
        std::fs::write(
            dir.join("requirements.toml"),
            "[[source]]\nid = \"s\"\n\n[[requirement]]\nid = \"r1\"\n\n[[requirement]]\nid = \"r2\"\n",
        )
        .unwrap();
        let store = EntryStore::at(&dir, EntryLayout::Combined);
        let ids = |k| {
            store
                .load(k)
                .unwrap()
                .iter()
                .map(|e| e.id().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(EntryKind::Requirement), ["r1", "r2"]);
        assert_eq!(ids(EntryKind::Source), ["s"]);
        assert!(ids(EntryKind::Decision).is_empty());
        assert!(!store.has_any(EntryKind::Decision));
    }

    #[test]
    fn load_errors_on_a_broken_file_and_load_lenient_skips_it() {
        let dir = tmp("broken");
        std::fs::write(dir.join("decisions.toml"), "[[decision]\n").unwrap();
        let store = EntryStore::at(&dir, EntryLayout::Combined);
        assert!(store.load(EntryKind::Decision).is_err());
        assert!(store.load_lenient(EntryKind::Decision).is_empty());
    }

    #[test]
    fn per_entry_layout_reads_one_file_per_entry_and_skips_other_files() {
        let dir = tmp("per-entry");
        std::fs::create_dir_all(dir.join("decisions")).unwrap();
        std::fs::write(
            dir.join("decisions/DEC-002.toml"),
            "[[decision]]\nid = \"DEC-002\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("decisions/DEC-001.toml"),
            "[[decision]]\nid = \"DEC-001\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("decisions/README.md"), "# not an entry\n").unwrap();
        let store = EntryStore::at(&dir, EntryLayout::PerEntry);
        let ids: Vec<String> = store
            .load(EntryKind::Decision)
            .unwrap()
            .iter()
            .map(|e| e.id().unwrap().to_string())
            .collect();
        assert_eq!(ids, ["DEC-001", "DEC-002"]);
        assert!(!store.has_any(EntryKind::OpenQuestion));
        assert_eq!(
            store.location_hint(EntryKind::OpenQuestion, "OQ-002"),
            "`specs/open_questions/OQ-002.toml`"
        );
        assert_eq!(
            EntryStore::at(&dir, EntryLayout::Combined)
                .location_hint(EntryKind::OpenQuestion, "OQ-002"),
            "`specs/open_questions.toml`"
        );
    }

    #[test]
    fn per_entry_layout_refuses_combined_files_with_content_and_skips_tombstones() {
        let dir = tmp("legacy");
        std::fs::write(
            dir.join("open_questions.toml"),
            "[[open_question]]\nid = \"OQ-001\"\nstatus = \"open\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("decisions.toml"),
            "MOVED to specs/decisions/ by specdev-analyzer migrate — upgrade specdev-analyzer to 0.3.0 or later\n\
             This file is not TOML on purpose: an analyzer that reads the old layout stops here.\n",
        )
        .unwrap();
        // Comments only, as the workspace bootstrap template writes: no entry
        // to lose.
        std::fs::write(dir.join("requirements.toml"), "# Requirements go here.\n").unwrap();
        let store = EntryStore::at(&dir, EntryLayout::PerEntry);
        assert_eq!(store.legacy_content(), [dir.join("open_questions.toml")]);
        let err = store.load(EntryKind::OpenQuestion).unwrap_err().to_string();
        assert!(
            err.contains("ENTRY-LEGACY-001") && err.contains("migrate"),
            "{err}"
        );
        assert!(store.load(EntryKind::Decision).unwrap().is_empty());
        // The combined layout reads the same file as its own.
        assert!(EntryStore::at(&dir, EntryLayout::Combined)
            .legacy_content()
            .is_empty());
    }

    /// No file in `src/` other than this one names an entry file, so a
    /// layout change is a change here only. Comments and doc comments may
    /// name them; code, messages and paths may not.
    #[test]
    fn entry_file_names_live_in_one_place() {
        let names = ["decisions.toml", "requirements.toml", "open_questions.toml"];
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut hits = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if path.file_name() != Some(std::ffi::OsStr::new("embedded")) {
                        stack.push(path);
                    }
                    continue;
                }
                if path.extension() != Some(std::ffi::OsStr::new("rs"))
                    || path.ends_with("src/entries.rs")
                {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (n, line) in text.lines().enumerate() {
                    let code = line.trim_start();
                    if code.starts_with("//") {
                        continue;
                    }
                    if names.iter().any(|name| code.contains(name)) {
                        let rel = path.strip_prefix(&src).unwrap().display();
                        hits.push(format!("src/{rel}:{}: {}", n + 1, code));
                    }
                }
            }
        }
        assert!(
            hits.is_empty(),
            "entry file names outside src/entries.rs — go through EntryStore:\n{}",
            hits.join("\n")
        );
    }
}
