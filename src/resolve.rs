//! Context resolver — produces the set of paths every skill and every
//! analyzer invocation must use, following decision D13:
//!
//!   1. Explicit path in `spec.toml` `[paths.overrides]`
//!   2. Project-local `.specdev/<kind>/<name>/`
//!   3. Plugin-shipped default at `<plugin_root>/<kind>/<name>/`
//!
//! The resolver emits a stable, human-readable markdown report that skills
//! inject via Claude Code's `!` command-substitution syntax.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::SpecConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Override,
    LocalSpecdev,
    PluginDefault,
    Missing,
}

impl Source {
    fn label(self) -> &'static str {
        match self {
            Source::Override => "spec.toml override",
            Source::LocalSpecdev => ".specdev",
            Source::PluginDefault => "plugin default",
            Source::Missing => "missing",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub kind: &'static str,
    pub name: String,
    pub path: PathBuf,
    pub source: Source,
}

#[derive(Debug, Clone)]
pub struct ResolvedContext {
    pub project_root: PathBuf,
    pub spec_toml: PathBuf,
    pub plugin_root: Option<PathBuf>,
    pub domain: String,
    pub language: String,
    pub codegen_name: String,
    pub spec_version: String,
    pub schema_major: String,
    pub schema: Resolved,
    pub domain_dir: Resolved,
    pub language_dir: Resolved,
    pub codegen_dir: Resolved,
}

pub const MISSING_SPEC_MARKER: &str = "SPECDEV_MISSING_SPEC_TOML";

pub fn is_missing_spec(err: &anyhow::Error) -> bool {
    format!("{err:#}").contains(MISSING_SPEC_MARKER)
}

pub fn resolve(project: &Path, plugin_root: Option<&Path>) -> Result<ResolvedContext> {
    let spec_toml = crate::config::find_spec_toml(project).with_context(|| {
        format!("spec.toml not found — run /spec-init or cd into a project directory [{MISSING_SPEC_MARKER}]")
    })?;
    let project_root = spec_toml
        .parent()
        .map(|p| p.to_path_buf())
        .expect("spec.toml has a parent directory");
    let cfg = SpecConfig::load(&spec_toml)?;

    let domain = cfg.project.domain.clone();
    let language = cfg.project.language.clone();
    let codegen_name = format!("{}-{}", domain, language);
    let schema_major = cfg.schema_major();

    let local_specdev = project_root.join(".specdev");

    let schema = resolve_kind(
        "schema",
        &format!("{}/{}", domain, schema_major),
        cfg.paths.overrides.schema.as_deref(),
        &project_root,
        &local_specdev,
        plugin_root,
    );
    let domain_dir = resolve_kind(
        "domains",
        &domain,
        cfg.paths.overrides.domain.as_deref(),
        &project_root,
        &local_specdev,
        plugin_root,
    );
    let language_dir = resolve_kind(
        "languages",
        &language,
        cfg.paths.overrides.language.as_deref(),
        &project_root,
        &local_specdev,
        plugin_root,
    );
    let codegen_dir = resolve_kind(
        "codegen",
        &codegen_name,
        cfg.paths.overrides.codegen.as_deref(),
        &project_root,
        &local_specdev,
        plugin_root,
    );

    Ok(ResolvedContext {
        project_root,
        spec_toml,
        plugin_root: plugin_root.map(|p| p.to_path_buf()),
        domain,
        language,
        codegen_name,
        spec_version: cfg.spec_version,
        schema_major,
        schema,
        domain_dir,
        language_dir,
        codegen_dir,
    })
}

fn resolve_kind(
    kind: &'static str,
    name: &str,
    override_path: Option<&str>,
    project_root: &Path,
    local_specdev: &Path,
    plugin_root: Option<&Path>,
) -> Resolved {
    if let Some(rel) = override_path {
        let path = project_root.join(rel);
        return Resolved {
            kind,
            name: name.to_string(),
            path,
            source: Source::Override,
        };
    }
    let local = local_specdev.join(kind).join(name);
    if local.exists() {
        return Resolved {
            kind,
            name: name.to_string(),
            path: local,
            source: Source::LocalSpecdev,
        };
    }
    if let Some(root) = plugin_root {
        return Resolved {
            kind,
            name: name.to_string(),
            path: root.join(kind).join(name),
            source: Source::PluginDefault,
        };
    }
    Resolved {
        kind,
        name: name.to_string(),
        path: PathBuf::new(),
        source: Source::Missing,
    }
}

pub fn render(ctx: &ResolvedContext) -> String {
    let mut out = String::new();
    out.push_str("## Spec Context\n\n");
    out.push_str(&format!("- Project root: `{}`\n", ctx.project_root.display()));
    out.push_str(&format!("- spec.toml: `{}`\n", ctx.spec_toml.display()));
    out.push_str(&format!("- Domain: `{}`\n", ctx.domain));
    out.push_str(&format!("- Language: `{}`\n", ctx.language));
    out.push_str(&format!("- Codegen name: `{}`\n", ctx.codegen_name));
    out.push_str(&format!(
        "- spec_version: `{}` (schema dir: `{}`)\n",
        ctx.spec_version, ctx.schema_major
    ));
    if let Some(root) = &ctx.plugin_root {
        out.push_str(&format!("- Plugin root: `{}`\n", root.display()));
    } else {
        out.push_str("- Plugin root: (unset — CLAUDE_PLUGIN_ROOT not provided)\n");
    }
    out.push('\n');
    out.push_str("## Resolved Paths\n\n");
    for r in [&ctx.schema, &ctx.domain_dir, &ctx.language_dir, &ctx.codegen_dir] {
        out.push_str(&format!(
            "- {:<8} `{}`  _(from {})_\n",
            format!("{}:", cap(r.kind)),
            r.path.display(),
            r.source.label()
        ));
    }
    out.push('\n');
    out.push_str("## Notes\n\n");
    out.push_str(
        "- Skills should read language/codegen/domain guidance from the resolved paths above,\n",
    );
    out.push_str(
        "  not from hard-coded plugin locations. This honors `.specdev/` overrides and\n",
    );
    out.push_str("  `[paths.overrides]` entries in `spec.toml`.\n");
    out
}

pub fn render_missing(project: &Path, plugin_root: Option<&Path>) -> String {
    let mut out = String::new();
    out.push_str("## Spec Context\n\n");
    out.push_str("- **Status:** no `spec.toml` found in this project.\n");
    out.push_str(&format!("- Working directory: `{}`\n", project.display()));
    if let Some(root) = plugin_root {
        out.push_str(&format!("- Plugin root: `{}`\n", root.display()));
    } else {
        out.push_str("- Plugin root: (unset — CLAUDE_PLUGIN_ROOT not provided)\n");
    }
    out.push('\n');
    out.push_str("## Notes\n\n");
    out.push_str(
        "- The project has not been initialized yet. Skills that need a resolved\n",
    );
    out.push_str(
        "  schema/domain/language/codegen path cannot operate. If this skill can\n",
    );
    out.push_str(
        "  bootstrap the project (e.g. `/spec-init`, `/spec-reverse`), proceed by\n",
    );
    out.push_str("  running the init flow first. Otherwise, stop and tell the user to run `/spec-init`.\n");
    out
}

fn cap(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(ch) => ch.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "specdev-resolve-{}-{}-{}",
            tag,
            std::process::id(),
            rand_suffix()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn rand_suffix() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64
    }

    fn write_spec(
        project: &Path,
        domain: &str,
        language: &str,
        extra: &str,
    ) {
        let toml = format!(
            r#"spec_version = "1.0"
[project]
name = "test"
domain = "{domain}"
language = "{language}"
{extra}
"#
        );
        fs::write(project.join("spec.toml"), toml).unwrap();
    }

    #[test]
    fn plugin_default_when_no_overrides() {
        let project = tmp("default");
        let plugin_root = tmp("plugin");
        write_spec(&project, "firmware", "rust", "");

        let ctx = resolve(&project, Some(&plugin_root)).unwrap();
        assert_eq!(ctx.schema.source, Source::PluginDefault);
        assert_eq!(ctx.domain_dir.source, Source::PluginDefault);
        assert_eq!(ctx.language_dir.source, Source::PluginDefault);
        assert_eq!(ctx.codegen_dir.source, Source::PluginDefault);
        assert!(ctx
            .schema
            .path
            .ends_with("schema/firmware/v1"));
        assert!(ctx.codegen_dir.path.ends_with("codegen/firmware-rust"));
    }

    #[test]
    fn local_specdev_wins_over_plugin() {
        let project = tmp("local");
        let plugin_root = tmp("plugin-local");
        write_spec(&project, "firmware", "rust", "");
        fs::create_dir_all(project.join(".specdev/languages/rust")).unwrap();

        let ctx = resolve(&project, Some(&plugin_root)).unwrap();
        assert_eq!(ctx.language_dir.source, Source::LocalSpecdev);
        assert!(ctx.language_dir.path.ends_with(".specdev/languages/rust"));
        // The other kinds (no local dir) fall through to plugin default.
        assert_eq!(ctx.schema.source, Source::PluginDefault);
    }

    #[test]
    fn explicit_override_wins_over_local_specdev() {
        let project = tmp("explicit");
        let plugin_root = tmp("plugin-explicit");
        write_spec(
            &project,
            "firmware",
            "rust",
            "[paths.overrides]\nlanguage = \"./custom/rust\"\n",
        );
        fs::create_dir_all(project.join(".specdev/languages/rust")).unwrap();
        fs::create_dir_all(project.join("custom/rust")).unwrap();

        let ctx = resolve(&project, Some(&plugin_root)).unwrap();
        assert_eq!(ctx.language_dir.source, Source::Override);
        assert!(ctx.language_dir.path.ends_with("custom/rust"));
    }
}
