//! Plugin-author lint: every unit's implementation guidance.
//!
//! A block, bridge or profile ships `guidance.md` at its root: the stable
//! rules an agent follows when it edits code in a project that uses the
//! unit, which no check can verify. It is inlined into the project, so it is
//! also what the project's reviewers read and may override.
//!
//! - **`GUIDANCE-001`** — the unit has no `guidance.md`. Required, so a
//!   unit's file set stays stable (`inline --merge` refuses a changed set).
//! - **`GUIDANCE-002`** — the guidance holds fenced code. Rules only; code
//!   in guidance is a template for the agent to copy, and it rots against
//!   the crate it names.
//! - **`GUIDANCE-003`** — the guidance is over [`GUIDANCE_CAP_BYTES`].
//! - **`PROSE-PROFILE-001`** — a block's or bridge's guidance names a
//!   profile. A reusable unit layers on "the project's profile"
//!   generically. The profile names are discovered from the plugin tree's
//!   `profiles/` directory, so the check holds no compile-time knowledge of
//!   any profile. A profile's own guidance may name profiles.
//!
//! Escape hatch for `PROSE-PROFILE-001`: a line containing `PROFILE-REF-OK`
//! is exempt, for a unit that is intentionally profile-bound and says so.
//!
//! The unit's `README.md` is author-facing catalog text that is never
//! inlined, so a discovery cross-reference there is fine and not checked.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::inline::GUIDANCE_FILE;

const ESCAPE: &str = "PROFILE-REF-OK";

/// The size cap for one unit's guidance, in bytes: the largest measured
/// unit (about 3.1 KB) plus about 30%.
pub const GUIDANCE_CAP_BYTES: usize = 4096;

/// Lint every block, bridge and profile under `plugin_root`. Returns sorted,
/// pre-coded offender lines (`<unit>: [CODE] ...`).
pub fn lint_guidance(plugin_root: &Path) -> Result<Vec<String>> {
    let profiles = unit_dirs(plugin_root, "profiles")?;
    let profile_names: Vec<String> = profiles.iter().filter_map(|p| dir_name(p)).collect();
    let mut offenders = Vec::new();
    for kind in ["blocks", "bridges", "profiles"] {
        for unit in unit_dirs(plugin_root, kind)? {
            let names: &[String] = if kind == "profiles" { &[] } else { &profile_names };
            scan_unit(&unit, names, &mut offenders)?;
        }
    }
    offenders.sort();
    offenders.dedup();
    Ok(offenders)
}

/// Directories under `<plugin_root>/<kind>/`, sorted. Empty when absent.
fn unit_dirs(plugin_root: &Path, kind: &str) -> Result<Vec<PathBuf>> {
    let base = plugin_root.join(kind);
    if !base.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&base)
        .with_context(|| format!("reading {}", base.display()))?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn dir_name(p: &Path) -> Option<String> {
    p.file_name().and_then(|s| s.to_str()).map(str::to_string)
}

fn scan_unit(unit: &Path, profiles: &[String], offenders: &mut Vec<String>) -> Result<()> {
    let unit_name = dir_name(unit).unwrap_or_else(|| "?".to_string());
    let path = unit.join(GUIDANCE_FILE);
    if !path.is_file() {
        offenders.push(format!(
            "{unit_name}: [GUIDANCE-001] missing {GUIDANCE_FILE} — every block, bridge and profile ships \
             its implementation guidance (rules only, under {GUIDANCE_CAP_BYTES} bytes)"
        ));
        return Ok(());
    }
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    if text.len() > GUIDANCE_CAP_BYTES {
        offenders.push(format!(
            "{unit_name}: [GUIDANCE-003] {GUIDANCE_FILE} is {} bytes; the cap is {GUIDANCE_CAP_BYTES}",
            text.len()
        ));
    }
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            offenders.push(format!(
                "{unit_name}: [GUIDANCE-002] {GUIDANCE_FILE}:{} opens fenced code — guidance holds rules, not code",
                idx + 1
            ));
            break;
        }
    }
    for (idx, line) in text.lines().enumerate() {
        if line.contains(ESCAPE) {
            continue;
        }
        for prof in profiles {
            if line_names_profile(line, prof) {
                offenders.push(format!(
                    "{unit_name}: [PROSE-PROFILE-001] {GUIDANCE_FILE}:{} names profile '{prof}' — refer to \
                     \"the project's profile\" generically (or mark the line {ESCAPE} if the unit is \
                     intentionally profile-bound)",
                    idx + 1
                ));
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

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("guidance_lint_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Two profiles, one block, each with clean guidance.
    fn scaffold(root: &Path) {
        for p in ["profiles/audio-plugin", "profiles/firmware-layered", "blocks/egui"] {
            write(&root.join(p).join(GUIDANCE_FILE), "# guidance\n\n- A rule.\n");
        }
    }

    #[test]
    fn a_clean_tree_passes() {
        let root = tmp("clean");
        scaffold(&root);
        assert!(lint_guidance(&root).unwrap().is_empty());
    }

    #[test]
    fn flags_a_profile_name_in_block_guidance_only() {
        let root = tmp("flag");
        scaffold(&root);
        write(&root.join("blocks/egui").join(GUIDANCE_FILE), "- Layer on the audio-plugin profile.\n");
        write(&root.join("profiles/audio-plugin").join(GUIDANCE_FILE), "- The audio-plugin crate is a cdylib.\n");
        let offenders = lint_guidance(&root).unwrap();
        assert_eq!(offenders.len(), 1, "{offenders:?}");
        assert!(offenders[0].starts_with("egui: [PROSE-PROFILE-001]"), "{offenders:?}");
    }

    #[test]
    fn generic_phrasing_and_the_escape_pass() {
        let root = tmp("generic");
        scaffold(&root);
        write(
            &root.join("blocks/egui").join(GUIDANCE_FILE),
            "- Layer on the project's profile, an audio plugin.\n- Bound to audio-plugin. PROFILE-REF-OK\n",
        );
        assert!(lint_guidance(&root).unwrap().is_empty());
    }

    #[test]
    fn missing_fenced_and_oversized_guidance_are_flagged() {
        let root = tmp("shape");
        scaffold(&root);
        fs::remove_file(root.join("profiles/firmware-layered").join(GUIDANCE_FILE)).unwrap();
        write(&root.join("blocks/egui").join(GUIDANCE_FILE), &format!("~~~\nx\n~~~\n{}", "y".repeat(5000)));
        let offenders = lint_guidance(&root).unwrap();
        assert!(offenders.iter().any(|o| o.starts_with("firmware-layered: [GUIDANCE-001]")), "{offenders:?}");
        assert!(offenders.iter().any(|o| o.starts_with("egui: [GUIDANCE-002]")), "{offenders:?}");
        assert!(offenders.iter().any(|o| o.starts_with("egui: [GUIDANCE-003]")), "{offenders:?}");
    }
}
