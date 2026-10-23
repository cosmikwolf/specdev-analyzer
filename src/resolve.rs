//! Skill-facing context printer.
//!
//! The prior cascade (`[paths.overrides]` → `.specdev/` → plugin defaults) is
//! retired at analyze time. What remains is a thin shim: skills (via
//! `scripts/resolve-context`) invoke `specdev-analyzer resolve` to discover
//! where a project lives. The implementation prints the discovered
//! [`LayoutContext`] as YAML and exits 0.

use std::path::Path;

use anyhow::Result;
use serde::Serialize;

use crate::layout::LayoutContext;

/// Discover and serialize the project's layout. Skill consumers parse the
/// returned YAML.
pub fn run(start: &Path, explicit_spec_path: Option<&Path>) -> Result<String> {
    let ctx = LayoutContext::discover(start, explicit_spec_path)?;
    let snapshot = LayoutSnapshot::from(&ctx);
    Ok(serde_yaml::to_string(&snapshot)?)
}

#[derive(Debug, Serialize)]
struct LayoutSnapshot<'a> {
    project_root: String,
    spec_toml: String,
    project_name: &'a str,
    profile: &'a str,
    language: &'a str,
    specs_dir: String,
    code_root: String,
    source_dir: String,
    specdev_root: String,
    schemas_dir: String,
    module_schema: String,
    inlined_blocks_dir: String,
    inlined_bridges_dir: String,
    inlined_languages_dir: String,
    reductions_dir: String,
    project_reduction: String,
    reductions_blocks_dir: String,
    reductions_bridges_dir: String,
    reductions_bridges_local_dir: String,
    domain_toml: String,
    modules_dir: String,
}

impl<'a> From<&'a LayoutContext> for LayoutSnapshot<'a> {
    fn from(ctx: &'a LayoutContext) -> Self {
        Self {
            project_root: ctx.project_root.display().to_string(),
            spec_toml: ctx.spec_toml_path.display().to_string(),
            project_name: &ctx.spec.project.name,
            profile: &ctx.spec.project.profile,
            language: &ctx.spec.project.language,
            specs_dir: ctx.specs_dir.display().to_string(),
            code_root: ctx.code_root.display().to_string(),
            source_dir: ctx.source_dir.display().to_string(),
            specdev_root: ctx.specdev_root.display().to_string(),
            schemas_dir: ctx.schemas_dir.display().to_string(),
            module_schema: ctx.module_schema_path.display().to_string(),
            inlined_blocks_dir: ctx.inlined_blocks_dir.display().to_string(),
            inlined_bridges_dir: ctx.inlined_bridges_dir.display().to_string(),
            inlined_languages_dir: ctx.inlined_languages_dir.display().to_string(),
            reductions_dir: ctx.reductions_dir.display().to_string(),
            project_reduction: ctx.project_reduction_path.display().to_string(),
            reductions_blocks_dir: ctx.reductions_blocks_dir.display().to_string(),
            reductions_bridges_dir: ctx.reductions_bridges_dir.display().to_string(),
            reductions_bridges_local_dir: ctx.reductions_bridges_local_dir.display().to_string(),
            domain_toml: ctx.domain_toml_path.display().to_string(),
            modules_dir: ctx.modules_dir.display().to_string(),
        }
    }
}
