//! Write decisions, requirements, sources and open questions into a
//! `spec_version = "2.0"` fixture, which keeps one file per entry.
//!
//! A test keeps its ledger text in the combined `1.x` shape (one string,
//! many `[[requirement]]` tables): [`write`] cuts it into per-entry files
//! with the analyzer's own `entries::plan_split`, so the fixture holds
//! exactly what `migrate` would have written. Writing a ledger replaces the
//! entries of its kinds, as overwriting the combined file did.

#![allow(dead_code)]

use std::path::Path;

/// The per-entry directories a combined file's kinds live in.
fn dirs_of(combined: &str) -> &'static [&'static str] {
    match combined {
        "requirements.toml" => &["requirements", "sources"],
        "decisions.toml" => &["decisions"],
        "open_questions.toml" => &["open_questions"],
        other => panic!("not a combined ledger file: {other}"),
    }
}

/// Replace the entries a combined file `combined` (`requirements.toml`,
/// `decisions.toml` or `open_questions.toml`) would hold with `text`'s.
pub fn write(project: &Path, combined: &str, text: impl AsRef<str>) {
    let text = text.as_ref();
    let specs = project.join("specs");
    clear(project, combined);
    std::fs::write(specs.join(combined), text).unwrap();
    let plan = specdev_analyzer::entries::plan_split(&specs)
        .unwrap_or_else(|e| panic!("splitting the fixture's {combined}: {e:#}"));
    for (rel, bytes) in plan.writes {
        let p = specs.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
    std::fs::remove_file(specs.join(combined)).unwrap();
}

/// Remove every entry file of `combined`'s kinds.
pub fn clear(project: &Path, combined: &str) {
    for dir in dirs_of(combined) {
        let d = project.join("specs").join(dir);
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "toml") {
                    std::fs::remove_file(p).unwrap();
                }
            }
        }
    }
}
