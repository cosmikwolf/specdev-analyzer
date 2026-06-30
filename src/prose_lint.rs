//! Plugin-author lint: inlined codegen prose must be profile-agnostic.
//!
//! - **`PROSE-PROFILE-001`** — a block's or bridge's `codegen/<lang>/README.md`
//!   (the prose that IS inlined into user projects and drives codegen) must not
//!   name a profile. The set of profile names is discovered from the plugin
//!   tree's `profiles/` directory, so the check holds zero compile-time
//!   knowledge of any specific profile — the same agnosticism discipline the
//!   analyzer holds for its own source, here extended to inlined content. A
//!   reusable block/bridge layers on "the project's profile" generically;
//!   hardcoding a profile name bakes in the composition it was authored
//!   against.
//!
//! Escape hatch: any line containing `PROFILE-REF-OK` is exempt, for a unit
//! that is intentionally profile-bound and says so visibly (mirrors the
//! `PLANNING-OK` convention used by the agnosticism lint).
//!
//! Scope: only `codegen/<lang>/README.md` — the inlined surface. The unit's
//! top-level `README.md` is author-facing catalog documentation that `inline`
//! never copies into a user project, so a discovery cross-reference there
//! ("pair with the X block") is fine and not checked.

use std::path::Path;

use anyhow::{Context, Result};

const ESCAPE: &str = "PROFILE-REF-OK";

/// Discover profile names from `profiles/`, then scan every
/// `{blocks,bridges}/*/codegen/<lang>/README.md` for a profile name used
/// outside an escape-marked line. Returns sorted, de-duplicated, pre-coded
/// offender lines (`<unit>: [PROSE-PROFILE-001] ...`). Agnostic: the profile
/// names are read from the tree, never hardcoded.
pub fn lint_codegen_prose_profiles(plugin_root: &Path) -> Result<Vec<String>> {
    let profiles = profile_names(plugin_root)?;
    let mut offenders = Vec::new();
    if profiles.is_empty() {
        return Ok(offenders);
    }
    for kind_dir in ["blocks", "bridges"] {
        let base = plugin_root.join(kind_dir);
        if !base.is_dir() {
            continue;
        }
        let mut units: Vec<_> = std::fs::read_dir(&base)
            .with_context(|| format!("reading {}", base.display()))?
            .filter_map(|r| r.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        units.sort();
        for unit in units {
            scan_unit(&unit, &profiles, &mut offenders)?;
        }
    }
    offenders.sort();
    offenders.dedup();
    Ok(offenders)
}

/// Directory names under `profiles/`. Empty (and the lint becomes a no-op) when
/// the tree ships no profiles.
fn profile_names(plugin_root: &Path) -> Result<Vec<String>> {
    let base = plugin_root.join("profiles");
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = std::fs::read_dir(&base)
        .with_context(|| format!("reading {}", base.display()))?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter_map(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .collect();
    names.sort();
    Ok(names)
}

fn scan_unit(unit: &Path, profiles: &[String], offenders: &mut Vec<String>) -> Result<()> {
    let unit_name = unit
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?")
        .to_string();
    let codegen = unit.join("codegen");
    if !codegen.is_dir() {
        return Ok(());
    }
    let mut langs: Vec<_> = std::fs::read_dir(&codegen)
        .with_context(|| format!("reading {}", codegen.display()))?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    langs.sort();
    for lang_dir in langs {
        let lang = lang_dir
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let readme = lang_dir.join("README.md");
        if !readme.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&readme)
            .with_context(|| format!("reading {}", readme.display()))?;
        for (idx, line) in text.lines().enumerate() {
            if line.contains(ESCAPE) {
                continue;
            }
            for prof in profiles {
                if line_names_profile(line, prof) {
                    offenders.push(format!(
                        "{unit_name}: [PROSE-PROFILE-001] codegen/{lang}/README.md:{} \
                         names profile '{prof}' — refer to \"the project's profile\" \
                         generically (or mark the line {ESCAPE} if the unit is \
                         intentionally profile-bound)",
                        idx + 1
                    ));
                    // One finding per (line, profile) is enough; keep scanning
                    // other profiles on the same line.
                }
            }
        }
    }
    Ok(())
}

/// True when `name` occurs in `line` not flanked by an identifier byte or a
/// hyphen. The profile names are hyphenated (`audio-plugin`), so this matches
/// the profile reference while leaving the space-separated domain phrase
/// (`audio plugin`) and longer hyphenated tokens (`firmware-layered-x`) alone.
fn line_names_profile(line: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let bytes = line.as_bytes();
    let nlen = name.len();
    let mut start = 0;
    while let Some(pos) = line[start..].find(name) {
        let s = start + pos;
        let e = s + nlen;
        let before_ok = s == 0 || !is_word_byte(bytes[s - 1]);
        let after_ok = e == bytes.len() || !is_word_byte(bytes[e]);
        if before_ok && after_ok {
            return true;
        }
        start = s + 1;
    }
    false
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(p: &Path, body: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("prose_lint_{tag}"));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn scaffold(root: &Path) {
        // Two profiles → two names the lint will learn from the tree.
        fs::create_dir_all(root.join("profiles/audio-plugin")).unwrap();
        fs::create_dir_all(root.join("profiles/firmware-layered")).unwrap();
    }

    #[test]
    fn flags_a_profile_name_in_block_codegen_prose() {
        let root = tmp("flag");
        scaffold(&root);
        write(
            &root.join("blocks/egui/codegen/rust/README.md"),
            "They layer on top of the audio-plugin profile's conventions.\n",
        );
        let offenders = lint_codegen_prose_profiles(&root).unwrap();
        assert_eq!(offenders.len(), 1, "{offenders:?}");
        assert!(offenders[0].contains("[PROSE-PROFILE-001]"));
        assert!(offenders[0].contains("egui"));
        assert!(offenders[0].contains("audio-plugin"));
    }

    #[test]
    fn generic_phrasing_passes() {
        let root = tmp("generic");
        scaffold(&root);
        write(
            &root.join("blocks/egui/codegen/rust/README.md"),
            "They layer on top of the project's profile's conventions.\n\
             Ships as an audio plugin shell.\n",
        );
        // "the project's profile" + the space-separated domain phrase
        // "audio plugin" must NOT match the hyphenated profile name.
        assert!(lint_codegen_prose_profiles(&root).unwrap().is_empty());
    }

    #[test]
    fn escape_marker_exempts_a_line() {
        let root = tmp("escape");
        scaffold(&root);
        write(
            &root.join("blocks/special/codegen/rust/README.md"),
            "This block is firmware-layered-only by design. PROFILE-REF-OK\n",
        );
        assert!(lint_codegen_prose_profiles(&root).unwrap().is_empty());
    }

    #[test]
    fn checks_bridges_too_and_ignores_top_level_readme() {
        let root = tmp("bridge");
        scaffold(&root);
        // Inlined codegen prose under a bridge → checked.
        write(
            &root.join("bridges/egui+nih-plug/codegen/rust/README.md"),
            "Owned by the audio-plugin profile.\n",
        );
        // Author-facing top-level README → never inlined → not checked.
        write(
            &root.join("bridges/egui+nih-plug/README.md"),
            "Use with the audio-plugin profile.\n",
        );
        let offenders = lint_codegen_prose_profiles(&root).unwrap();
        assert_eq!(offenders.len(), 1, "{offenders:?}");
        assert!(offenders[0].contains("codegen/rust/README.md"));
    }

    #[test]
    fn no_profiles_dir_is_a_noop() {
        let root = tmp("noprofiles");
        write(
            &root.join("blocks/egui/codegen/rust/README.md"),
            "Names audio-plugin but there is no profiles/ dir to learn from.\n",
        );
        assert!(lint_codegen_prose_profiles(&root).unwrap().is_empty());
    }
}
