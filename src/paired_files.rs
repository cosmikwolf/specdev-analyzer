//! Profile-declared paired files.
//!
//! A profile may ship config files that a project needs exactly once —
//! `tsconfig.json`, `next.config.ts`, `.gitignore`, an `app/.gitkeep`
//! that makes a directory exist. They are declared as a top-level
//! `paired_files:` list in `profile.yaml`, read from the inlined
//! `<specdev_root>/templates/` copy, and written by `bootstrap` **only when
//! absent**. After that first write they belong to the project: never
//! rewritten. (`next build` rewrites `tsconfig.json` on its first run.)
//!
//! The only token substituted in a body is `{project_name}`, so a JSON or
//! TypeScript brace in a template is never a token.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::layout::LayoutContext;
use crate::manifest::Profile;

/// A `paired_files` target must be a relative path inside the project
/// root. Returns it as a `PathBuf`.
fn check_target(target: &str) -> Result<PathBuf> {
    let path = Path::new(target);
    let escapes = path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        });
    if escapes || target.trim().is_empty() {
        bail!(
            "profile.yaml paired_files target `{target}` must be a relative path inside \
             the project root (no leading `/`, no `..`)"
        );
    }
    Ok(path.to_path_buf())
}

/// Read the inlined profile's `paired_files` rows into `(target, body)`
/// pairs. `Ok(empty)` when there is no inlined profile or the profile
/// declares none. Errors: a target that is absolute or escapes the
/// project root (`..`), or a template file that cannot be read.
pub fn paired_files_from_profile(layout: &LayoutContext) -> Result<Vec<(PathBuf, String)>> {
    let Some(profile) = Profile::load_inlined(&layout.specdev_root)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(profile.paired_files.len());
    for row in &profile.paired_files {
        let target = check_target(&row.target)?;
        let template_path = layout.specdev_root.join(&row.template_file);
        let body = std::fs::read_to_string(&template_path).with_context(|| {
            format!(
                "reading paired_files template at {} (referenced from profile.yaml)",
                template_path.display()
            )
        })?;
        out.push((
            target,
            body.replace("{project_name}", &layout.spec.project.name),
        ));
    }
    Ok(out)
}

/// Pre-flight for `bootstrap`: check the plugin profile's `paired_files`
/// rows before anything is written — every target is a relative path inside
/// the project, no target is named twice, and every template exists in the
/// profile's own `templates/`. `Ok` when the profile declares none.
pub fn check_profile_rows(profile_dir: &Path) -> Result<()> {
    let manifest = profile_dir.join("profile.yaml");
    if !manifest.is_file() {
        return Ok(());
    }
    let profile = Profile::load(&manifest)?;
    let mut seen = BTreeSet::new();
    for row in &profile.paired_files {
        let target = check_target(&row.target)?;
        if !seen.insert(target.clone()) {
            bail!(
                "profile.yaml paired_files names `{}` twice; two writers for one path is refused",
                target.display()
            );
        }
        let template = profile_dir.join(&row.template_file);
        if !template.is_file() {
            bail!(
                "profile.yaml paired_files template `{}` does not exist at {}",
                row.template_file,
                template.display()
            );
        }
    }
    Ok(())
}

/// Write every declared paired file that does not exist yet, in declared
/// order; an existing file is left as it is. Returns the paths written,
/// project-root-relative. Refuses before writing anything when two rows
/// name one target.
pub fn write_absent(layout: &LayoutContext) -> Result<Vec<PathBuf>> {
    let rows = paired_files_from_profile(layout)?;
    let mut seen = BTreeSet::new();
    for (rel, _) in &rows {
        if !seen.insert(rel.clone()) {
            bail!(
                "profile.yaml paired_files names `{}` twice; two writers for one path is refused",
                rel.display()
            );
        }
    }
    let mut written = Vec::new();
    for (rel, body) in rows {
        let dest = layout.project_root.join(&rel);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&dest, body).with_context(|| format!("writing {}", dest.display()))?;
        written.push(rel);
    }
    Ok(written)
}
