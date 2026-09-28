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
//! Today every project keeps them in three files under `specs/`:
//! `decisions.toml` (`[[decision]]`), `requirements.toml` (`[[source]]` and
//! `[[requirement]]`) and `open_questions.toml` (`[[open_question]]`).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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

    /// The file under `specs/` that holds this kind. Requirements and
    /// sources share one.
    fn file_name(self) -> &'static str {
        match self {
            EntryKind::Decision => "decisions.toml",
            EntryKind::Requirement | EntryKind::Source => "requirements.toml",
            EntryKind::OpenQuestion => "open_questions.toml",
        }
    }
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
}

impl EntryStore {
    /// The project's own entries.
    pub fn for_layout(layout: &LayoutContext) -> Self {
        Self::at(&layout.specs_dir)
    }

    /// The entries under a `specs/` directory, e.g. a workspace root's.
    pub fn at(specs_dir: &Path) -> Self {
        Self {
            specs_dir: specs_dir.to_path_buf(),
        }
    }

    /// The files that exist and hold entries of `kind`.
    pub fn files(&self, kind: EntryKind) -> Vec<PathBuf> {
        let path = self.expected_file(kind);
        if path.is_file() {
            vec![path]
        } else {
            Vec::new()
        }
    }

    /// Where entries of `kind` are kept when there are any. Named in the
    /// "none authored yet" finding, which needs a path when [`files`] is
    /// empty.
    ///
    /// [`files`]: EntryStore::files
    pub fn expected_file(&self, kind: EntryKind) -> PathBuf {
        self.specs_dir.join(kind.file_name())
    }

    /// Whether any file holds entries of `kind`.
    pub fn has_any(&self, kind: EntryKind) -> bool {
        !self.files(kind).is_empty()
    }

    /// Every entry of `kind`, in file order. An unreadable or unparseable
    /// file is an error.
    pub fn load(&self, kind: EntryKind) -> Result<Vec<Entry>> {
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

    /// How a message names the place to edit entry `id`: the path relative
    /// to the project root, in backticks, e.g. `` `specs/open_questions.toml` ``.
    pub fn location_hint(kind: EntryKind, _id: &str) -> String {
        format!("`specs/{}`", kind.file_name())
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
        let store = EntryStore::at(&dir);
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
        let store = EntryStore::at(&dir);
        assert!(store.load(EntryKind::Decision).is_err());
        assert!(store.load_lenient(EntryKind::Decision).is_empty());
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
