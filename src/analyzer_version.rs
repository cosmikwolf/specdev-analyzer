//! Is this binary older than the plugin it runs beside?
//!
//! `contract_version` (in `profile.yaml`) catches a plugin whose manifest
//! *schema* moved past the binary. It cannot catch a binary that is behind
//! on *behaviour* inside the same contract — a fix the plugin's skills rely
//! on, such as a `gen-plan` verdict or an exit code. That binary ran silently
//! for months on one workstation (0.1.0 against a 0.2.0 plugin).
//!
//! The plugin states the analyzer version it is released with in one file,
//! [`MIN_VERSION_FILE`], holding one `MAJOR.MINOR.PATCH` line. On every
//! command this module reads it through the plugin root the caller already
//! named — `--plugin-root` when given (and only it), else
//! `SPECDEV_PLUGIN_ROOT` (which every skill exports first), else
//! `CLAUDE_PLUGIN_ROOT` — and prints a one-line warning on stderr when this
//! binary is older.
//!
//! The plugin keeps the file equal to the analyzer version its install
//! docs pin (`scripts/check-analyzer-pin` asserts it), so a user who
//! installed as documented never sees the warning.
//!
//! A warning, never a refusal: a user who cannot re-install at once can
//! still work, and a stale warning is harmless where a stale refusal would
//! block. Silent when no plugin root is named, or the plugin predates the
//! file — the check must cost nothing where it has nothing to compare.
//! `[paths].plugin_root` in `spec.toml` is not consulted: reading it needs
//! the project, and skills always export the environment variable.

use std::path::{Path, PathBuf};

use crate::plugin_root::PLUGIN_MARKER;

/// The plugin-relative file that names the analyzer version the plugin is
/// released with; an older binary warns. One line, `MAJOR.MINOR.PATCH`.
pub const MIN_VERSION_FILE: &str = "shared/min-analyzer-version";

/// This binary's version.
pub const THIS_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The warning to print, if any, for the plugin root the caller named.
pub fn check(plugin_flag: Option<&Path>) -> Option<String> {
    let root = named_plugin_root(plugin_flag)?;
    let path = root.join(MIN_VERSION_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    let wanted = text.trim();
    let Some(min) = parse(wanted) else {
        return Some(format!(
            "warning: [ANALYZER-VERSION-002] {} does not hold a MAJOR.MINOR.PATCH version \
             (read `{wanted}`); cannot check this specdev-analyzer ({THIS_VERSION}) against \
             the plugin.",
            path.display()
        ));
    };
    let this = parse(THIS_VERSION)?;
    // One line: it prints on every command, and `/spec-gen --all` runs
    // dozens.
    (this < min).then(|| {
        format!(
            "warning: [ANALYZER-VERSION-001] specdev-analyzer {THIS_VERSION} is older than \
             the {wanted} the specdev plugin needs; re-install it (see {})",
            root.join("shared/analyzer-install.md").display(),
        )
    })
}

/// The plugin root the caller named. A given `--plugin-root` is the only
/// candidate: falling through to the environment would check a plugin the
/// command does not use. Otherwise the first environment root carrying the
/// plugin marker. A named path without the marker is not reported here:
/// plugin-root errors belong to the commands that need the root
/// (`PLUGIN-001`).
fn named_plugin_root(plugin_flag: Option<&Path>) -> Option<PathBuf> {
    let is_plugin = |p: &PathBuf| p.join(PLUGIN_MARKER).is_file();
    if let Some(flag) = plugin_flag {
        return Some(flag.to_path_buf()).filter(is_plugin);
    }
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    [env("SPECDEV_PLUGIN_ROOT"), env("CLAUDE_PLUGIN_ROOT")]
        .into_iter()
        .flatten()
        .find(is_plugin)
}

/// `MAJOR.MINOR.PATCH`, ignoring a `-pre` or `+build` suffix on the patch.
/// So `0.3.0-dev` compares equal to `0.3.0`: a development build at the
/// next version is silent — a missed warning, never a false one.
fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let triple = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(triple)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_and_orders_numerically() {
        assert_eq!(parse("0.2.0"), Some((0, 2, 0)));
        assert_eq!(parse("1.10.3-dev"), Some((1, 10, 3)));
        assert!(parse("0.10.0") > parse("0.9.9"));
        assert_eq!(parse("0.2"), None);
        assert_eq!(parse("0.2.0.1"), None);
        assert_eq!(parse("v0.2.0"), None);
        assert_eq!(parse(""), None);
    }
}
