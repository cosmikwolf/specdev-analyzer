//! Plugin-root resolution for `bootstrap`, `inline`, and `inline-bridge`.
//!
//! Resolution order, highest precedence first:
//!
//! 1. `--plugin-root <path>` CLI flag.
//! 2. `[paths].plugin_root` in `spec.toml` (resolved relative to the manifest).
//! 3. `SPECDEV_PLUGIN_ROOT` environment variable.
//! 4. `CLAUDE_PLUGIN_ROOT` environment variable (set by Claude Code when the
//!    plugin is active).
//! 5. Error `PLUGIN-001` — none of the above resolve to a directory containing
//!    `.claude-plugin/plugin.json`.
//!
//! A path is only accepted as a plugin root if it contains the marker file
//! `.claude-plugin/plugin.json`. This catches the common mistake of pointing
//! at the wrong directory (e.g. a project root or `~/.claude/`).

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::config::SpecConfig;

/// Marker that a directory is the root of a Claude Code plugin tree.
pub const PLUGIN_MARKER: &str = ".claude-plugin/plugin.json";

/// Inputs from the CLI surface that affect plugin-root resolution.
#[derive(Debug, Default, Clone)]
pub struct PluginRootInputs {
    /// `--plugin-root <path>` CLI flag (highest precedence).
    pub flag: Option<PathBuf>,
}

/// Resolve the plugin root for `bootstrap`. Bootstrap has no `spec.toml`
/// yet (it is writing one), so the `[paths].plugin_root` step is skipped;
/// only the flag and environment variables apply.
pub fn resolve_for_bootstrap(inputs: &PluginRootInputs) -> Result<PathBuf> {
    if let Some(p) = &inputs.flag {
        return validate(p).with_context(|| {
            format!(
                "[PLUGIN-001] resolving plugin root from --plugin-root {}",
                p.display()
            )
        });
    }
    if let Some(env) = nonempty_env("SPECDEV_PLUGIN_ROOT") {
        return validate(&PathBuf::from(&env)).with_context(|| {
            format!("[PLUGIN-001] resolving plugin root from SPECDEV_PLUGIN_ROOT={env}")
        });
    }
    if let Some(env) = nonempty_env("CLAUDE_PLUGIN_ROOT") {
        return validate(&PathBuf::from(&env)).with_context(|| {
            format!("[PLUGIN-001] resolving plugin root from CLAUDE_PLUGIN_ROOT={env}")
        });
    }
    Err(anyhow!(plugin_root_unresolvable_message(false)))
}

/// Resolve the plugin root using the documented precedence chain.
///
/// `spec_path` is the absolute path to `spec.toml`; relative `plugin_root`
/// values in `spec.toml` are resolved against its parent directory.
pub fn resolve(
    inputs: &PluginRootInputs,
    spec_path: &Path,
    spec_config: &SpecConfig,
) -> Result<PathBuf> {
    // 1. Explicit flag.
    if let Some(p) = &inputs.flag {
        return validate(p).with_context(|| {
            format!(
                "[PLUGIN-001] resolving plugin root from --plugin-root {}",
                p.display()
            )
        });
    }

    // 2. Field in spec.toml.
    if let Some(rel) = &spec_config.paths.plugin_root {
        let manifest_dir = spec_path.parent().ok_or_else(|| {
            anyhow!(
                "[PLUGIN-001] spec.toml path {} has no parent",
                spec_path.display()
            )
        })?;
        let candidate = manifest_dir.join(rel);
        return validate(&candidate).with_context(|| {
            format!(
                "[PLUGIN-001] resolving plugin root from [paths].plugin_root = {rel:?} (relative to {})",
                manifest_dir.display()
            )
        });
    }

    // 3. SPECDEV_PLUGIN_ROOT.
    if let Some(env) = nonempty_env("SPECDEV_PLUGIN_ROOT") {
        return validate(&PathBuf::from(&env)).with_context(|| {
            format!("[PLUGIN-001] resolving plugin root from SPECDEV_PLUGIN_ROOT={env}")
        });
    }

    // 4. CLAUDE_PLUGIN_ROOT.
    if let Some(env) = nonempty_env("CLAUDE_PLUGIN_ROOT") {
        return validate(&PathBuf::from(&env)).with_context(|| {
            format!("[PLUGIN-001] resolving plugin root from CLAUDE_PLUGIN_ROOT={env}")
        });
    }

    Err(anyhow!(plugin_root_unresolvable_message(true)))
}

/// Build the [PLUGIN-001] error message. `with_spec_field = true` adds
/// the `[paths].plugin_root` line for callers that have a `spec.toml`
/// (i.e. `resolve`, not `resolve_for_bootstrap`).
///
/// Wording sharpened in the F-003 quick-wins cluster: the original
/// message implied `CLAUDE_PLUGIN_ROOT` is auto-set everywhere inside
/// Claude Code, but it is only injected into plugin/skill execution
/// contexts — Bash subshells (the analyzer's typical invocation surface)
/// do NOT inherit it. Saying "export SPECDEV_PLUGIN_ROOT once" is the
/// concrete fix; the parenthetical now names the inheritance gap.
fn plugin_root_unresolvable_message(with_spec_field: bool) -> String {
    let mut msg =
        String::from("[PLUGIN-001] plugin root not resolvable. Pass --plugin-root <path>");
    if with_spec_field {
        msg.push_str(", set [paths].plugin_root in spec.toml,");
    }
    msg.push_str(
        " or export SPECDEV_PLUGIN_ROOT=<path-to-specdev-plugin>. \
         CLAUDE_PLUGIN_ROOT is auto-set inside plugin/skill execution \
         contexts but is NOT inherited by Bash subshells the analyzer \
         is typically run from — capture it once at session start.",
    );
    msg
}

fn nonempty_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(s) if !s.is_empty() => Some(s),
        _ => None,
    }
}

fn validate(candidate: &Path) -> Result<PathBuf> {
    if !candidate.is_dir() {
        return Err(anyhow!("{} is not a directory", candidate.display()));
    }
    let marker = candidate.join(PLUGIN_MARKER);
    if !marker.is_file() {
        return Err(anyhow!(
            "{} is not a Claude Code plugin root (missing {})",
            candidate.display(),
            PLUGIN_MARKER,
        ));
    }
    std::fs::canonicalize(candidate)
        .with_context(|| format!("canonicalizing {}", candidate.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn mk_plugin(root: &Path) {
        fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        fs::write(root.join(PLUGIN_MARKER), "{}").unwrap();
    }

    fn mk_spec(dir: &Path, plugin_root_field: Option<&str>) -> (PathBuf, SpecConfig) {
        let manifest = dir.join("spec.toml");
        let mut text = String::from(
            r#"
spec_version = "1.0"

[project]
name = "demo"
profile = "firmware-layered"
language = "rust"

[paths]
specs = "specs"
source = "src"
"#,
        );
        if let Some(p) = plugin_root_field {
            text.push_str(&format!("plugin_root = {p:?}\n"));
        }
        fs::write(&manifest, &text).unwrap();
        let cfg = SpecConfig::parse(&manifest, &text).unwrap();
        (manifest, cfg)
    }

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir()
            .join("specdev-plugin-root-tests")
            .join(format!("{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn flag_wins_when_set() {
        let dir = tmp("flag");
        let plugin = dir.join("plug");
        mk_plugin(&plugin);
        let (manifest, cfg) = mk_spec(&dir, None);
        let inputs = PluginRootInputs {
            flag: Some(plugin.clone()),
        };
        let resolved = resolve(&inputs, &manifest, &cfg).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&plugin).unwrap());
    }

    #[test]
    fn rejects_dir_without_marker() {
        let dir = tmp("no-marker");
        let plugin = dir.join("plug");
        fs::create_dir_all(&plugin).unwrap(); // no marker
        let (manifest, cfg) = mk_spec(&dir, None);
        let inputs = PluginRootInputs { flag: Some(plugin) };
        let err = resolve(&inputs, &manifest, &cfg).unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("PLUGIN-001"), "{s}");
        assert!(s.contains(".claude-plugin/plugin.json"), "{s}");
    }

    #[test]
    fn spec_toml_field_used_when_no_flag() {
        let dir = tmp("from-spec");
        let plugin = dir.join("plug");
        mk_plugin(&plugin);
        let (manifest, cfg) = mk_spec(&dir, Some("plug"));
        let inputs = PluginRootInputs::default();
        let resolved = resolve(&inputs, &manifest, &cfg).unwrap();
        assert_eq!(resolved, std::fs::canonicalize(&plugin).unwrap());
    }

    #[test]
    fn errors_when_nothing_resolves() {
        let dir = tmp("none");
        let (manifest, cfg) = mk_spec(&dir, None);
        // Scrub the env so an ambient CLAUDE_PLUGIN_ROOT doesn't leak in.
        let saved_specdev = std::env::var("SPECDEV_PLUGIN_ROOT").ok();
        let saved_claude = std::env::var("CLAUDE_PLUGIN_ROOT").ok();
        std::env::remove_var("SPECDEV_PLUGIN_ROOT");
        std::env::remove_var("CLAUDE_PLUGIN_ROOT");

        let inputs = PluginRootInputs::default();
        let result = resolve(&inputs, &manifest, &cfg);

        if let Some(v) = saved_specdev {
            std::env::set_var("SPECDEV_PLUGIN_ROOT", v);
        }
        if let Some(v) = saved_claude {
            std::env::set_var("CLAUDE_PLUGIN_ROOT", v);
        }

        let err = result.unwrap_err();
        let s = format!("{err:#}");
        assert!(s.contains("PLUGIN-001"), "{s}");
    }
}
