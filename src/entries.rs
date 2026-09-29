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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Whether `text` is a tombstone `migrate` wrote: its first line starts with
/// [`TOMBSTONE_PREFIX`] and names `specdev-analyzer migrate`.
pub fn is_tombstone(text: &str) -> bool {
    let first = text.lines().next().unwrap_or("");
    first.starts_with(TOMBSTONE_PREFIX) && first.contains("by specdev-analyzer migrate")
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
                let Ok(read) = std::fs::read_dir(&dir) else {
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
            EntryLayout::PerEntry => format!("`specs/{}/{id}.toml`", kind.entry_dir()),
        }
    }
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
            "MOVED to specs/decisions/ by specdev-analyzer migrate — upgrade specdev-analyzer to 0.3.0\n",
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
