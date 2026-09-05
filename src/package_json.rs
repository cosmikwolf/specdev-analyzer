//! Deterministic package.json emission from spec metadata.
//!
//! The TypeScript sibling of `cargo_toml.rs` and `pubspec_yaml.rs` — a
//! concrete sibling emitter, not a shared rendering abstraction. The
//! three share merge *discipline* (user > blocks > bridges, first wins,
//! version and source conflicts warn, `test_only` routes to the dev
//! section), held in lockstep by a table-driven conformance test, not by
//! code. Reads `spec.toml` `[project]` + `architecture.toml` (`version`,
//! `[environment]`, `[scripts]`, `[[external_deps]]`) + module-level
//! `external_deps[]` + inlined block/bridge `deps.yaml` `npm:`
//! contributions, and emits a package.json whose content is a
//! deterministic projection of the spec.
//!
//! Every npm dep is in the spec; package.json is OUTPUT. The lockfile is
//! the package manager's and is never emitted or recorded.
//!
//! JSON specifics this emitter owns: fixed key order (`name, version,
//! description, private, packageManager, type, engines, scripts,
//! dependencies, devDependencies`), 2-space indent, trailing newline,
//! lex-sorted maps, empty maps omitted. A dependency value is ONE
//! string, so each reference carries exactly one shape: a registry range
//! in `version`, or one source key (`git` / `path` / `link` /
//! `workspace`) that owns the whole value.
//!
//! Workspace root (`[workspace]` in spec.toml): the root package.json
//! (`private: true`, tooling deps, root scripts) plus a gen-owned
//! sibling file `pnpm-workspace.yaml` carrying `packages:` in member
//! declaration order. The sibling is overwritten on every write — it is
//! output, like the manifest itself. pnpm also reads `overrides` and
//! `catalog` from that file; v1 emits `packages:` only and does not merge
//! hand-added keys.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};

use crate::layout::LayoutContext;
use crate::manifest::block_deps as bd_module;
use crate::manifest::{
    has_workspace_ancestor, BlockDeps, NpmSource, PackageJsonSection, PackageJsonShapeKind, Profile,
};

/// One package.json dependency entry, normalized. Spec-side twin of the
/// manifest loader's `NpmEntry` (which lacks `test_only` — block/bridge
/// contributions are runtime deps by rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NpmRef {
    package: String,
    version: Option<String>,
    source: Option<NpmSource>,
    test_only: bool,
}

/// Result of a package.json render: the rendered text, non-fatal
/// warnings, and gen-owned sibling files (`(relative path, body)`) that
/// the caller writes beside the manifest and **overwrites** on every
/// run. Today the only sibling is the workspace root's
/// `pnpm-workspace.yaml`.
#[derive(Debug, Clone)]
pub struct RenderedPackageJson {
    pub text: String,
    pub warnings: Vec<String>,
    pub sibling_files: Vec<(String, String)>,
}

/// The gen-owned sibling the workspace root emits beside package.json.
pub const WORKSPACE_FILE: &str = "pnpm-workspace.yaml";

/// Render package.json for the project at `layout`. Three cases,
/// mirroring the cargo and pub paths:
///
/// * **Workspace root** (`[workspace]` set in spec.toml) → root
///   package.json + `pnpm-workspace.yaml` sibling.
/// * **Workspace member** → a plain package.json; pnpm needs no member
///   marker (the root's `packages:` list is the membership).
/// * **Standalone** → per-shape body from the inlined profile.yaml's
///   `package_json` section. The section is required — a TypeScript
///   project without it is a configuration error, not a fallback case.
pub fn render(layout: &LayoutContext) -> Result<RenderedPackageJson> {
    let manifest = load_inlined_profile(layout)?;
    let section = manifest.package_json.as_ref().ok_or_else(|| {
        anyhow!(
            "profile.yaml at {} has no `package_json` section; cannot dispatch \
             package.json render (the profile must declare shape and defaults \
             for its language)",
            layout.specdev_root.join("profile.yaml").display(),
        )
    })?;

    if layout.spec.workspace.is_some() {
        return render_workspace_root(layout, section);
    }
    // Membership changes nothing in the member's own manifest; the walk
    // is kept so a member with a malformed ancestor spec.toml fails here,
    // where the pub path fails too.
    let _is_member = has_workspace_ancestor(layout.project_root.as_path())?;
    render_package(layout, section, false)
}

fn load_inlined_profile(layout: &LayoutContext) -> Result<Profile> {
    Profile::load_inlined(&layout.specdev_root)?.ok_or_else(|| {
        anyhow!(
            "no profile.yaml at {} — package.json emission is manifest-driven and \
             has no pre-manifest fallback; re-bootstrap or run `specdev migrate`",
            layout.specdev_root.join("profile.yaml").display(),
        )
    })
}

/// architecture.toml `[environment]` — closed key set (`node`,
/// `package_manager`); an unknown key is a hard error, never a silent
/// drop.
#[derive(Debug, Clone, Default)]
struct NpmEnvironment {
    node: Option<String>,
    package_manager: Option<String>,
}

/// The resolved header fields shared by the package and workspace-root
/// shapes: architecture overrides the profile defaults key by key.
struct Header {
    version: Option<String>,
    private: bool,
    package_manager: Option<String>,
    module_type: Option<&'static str>,
    engines_node: Option<String>,
    scripts: BTreeMap<String, String>,
}

fn resolve_header(
    section: &PackageJsonSection,
    arch: &toml::value::Table,
    private: bool,
) -> Result<Header> {
    let env = extract_environment(arch)?;
    let mut scripts = section.scripts.clone();
    for (k, v) in extract_scripts(arch)? {
        scripts.insert(k, v);
    }
    Ok(Header {
        version: extract_app_version(arch)?,
        private,
        package_manager: env
            .package_manager
            .or_else(|| section.package_manager.clone()),
        module_type: section.module_type.map(|t| t.as_str()),
        engines_node: env
            .node
            .or_else(|| section.engines.as_ref().and_then(|e| e.node.clone())),
        scripts,
    })
}

fn render_package(
    layout: &LayoutContext,
    section: &PackageJsonSection,
    _workspace_member: bool,
) -> Result<RenderedPackageJson> {
    let project = &layout.spec.project;
    let arch = read_architecture(layout)?;
    let mut warnings: Vec<String> = Vec::new();
    let deps = collect_all_deps(layout, &arch, &mut warnings)?;

    if !is_legal_npm_name(&project.name) {
        warnings.push(format!(
            "package name `{}` (from [project].name) is not a legal npm package \
             name (lowercase, URL-safe, no leading `.` or `_`, at most 214 \
             characters, optional `@scope/`); the package manager will reject \
             the emitted package.json until the project is renamed",
            project.name,
        ));
    }

    let header = resolve_header(
        section,
        &arch,
        matches!(section.shape, PackageJsonShapeKind::App),
    )?;
    let text = write_package_json(
        &project.name,
        project.description.as_deref(),
        &header,
        &deps,
    );
    Ok(RenderedPackageJson {
        text,
        warnings,
        sibling_files: Vec::new(),
    })
}

/// The pnpm workspace root: a `private` package.json (never published;
/// tooling deps and root scripts only) plus `pnpm-workspace.yaml`
/// listing the members in declaration order.
fn render_workspace_root(
    layout: &LayoutContext,
    section: &PackageJsonSection,
) -> Result<RenderedPackageJson> {
    let workspace_section = layout
        .spec
        .workspace
        .as_ref()
        .expect("render_workspace_root only called when [workspace] is set");
    let project = &layout.spec.project;
    let arch = read_architecture(layout)?;
    let mut warnings: Vec<String> = Vec::new();
    let deps = collect_all_deps(layout, &arch, &mut warnings)?;

    let header = resolve_header(section, &arch, true)?;
    let text = write_package_json(
        &project.name,
        project.description.as_deref(),
        &header,
        &deps,
    );

    let mut ws = String::from("packages:\n");
    for member in &workspace_section.members {
        ws.push_str(&format!("  - {}\n", yaml_quote(&member.path)));
    }

    Ok(RenderedPackageJson {
        text,
        warnings,
        sibling_files: vec![(WORKSPACE_FILE.to_string(), ws)],
    })
}

// ---------------------------------------------------------------------------
// Input gathering
// ---------------------------------------------------------------------------

fn read_architecture(layout: &LayoutContext) -> Result<toml::value::Table> {
    let path = &layout.architecture_toml_path;
    if !path.is_file() {
        return Ok(toml::value::Table::new());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value: toml::Value =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    match value {
        toml::Value::Table(t) => Ok(t),
        _ => Err(anyhow!("{} did not parse as a TOML table", path.display())),
    }
}

/// Merge order: user (architecture `[[external_deps]]`, then per-module
/// `external_deps[]`) > blocks > bridges. First insert wins; conflicts
/// warn.
fn collect_all_deps(
    layout: &LayoutContext,
    arch: &toml::value::Table,
    warnings: &mut Vec<String>,
) -> Result<BTreeMap<String, NpmRef>> {
    let mut deps: BTreeMap<String, NpmRef> = BTreeMap::new();
    for r in extract_npm_ref_array(arch.get("external_deps"))? {
        merge_dep(&mut deps, r, warnings);
    }
    for r in collect_module_external_deps(layout)? {
        merge_dep(&mut deps, r, warnings);
    }
    for r in collect_inlined_unit_deps(layout, &layout.inlined_blocks_dir, "blocks")? {
        merge_dep(&mut deps, r, warnings);
    }
    for r in collect_inlined_unit_deps(layout, &layout.inlined_bridges_dir, "bridges")? {
        merge_dep(&mut deps, r, warnings);
    }
    Ok(deps)
}

/// Read architecture.toml's optional top-level `version` (the
/// package.json `version` field). Type errors are hard failures;
/// absence is fine — the package manager installs a versionless
/// manifest.
fn extract_app_version(arch: &toml::value::Table) -> Result<Option<String>> {
    match arch.get("version") {
        None => Ok(None),
        Some(toml::Value::String(s)) if s.is_empty() => {
            Err(anyhow!("architecture.toml `version` must be non-empty"))
        }
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(anyhow!(
            "architecture.toml `version` must be a string (e.g. \"0.1.0\"); got {}",
            other.type_str(),
        )),
    }
}

/// Read architecture.toml's optional `[environment]` table (closed key
/// set: `node`, `package_manager`).
fn extract_environment(arch: &toml::value::Table) -> Result<NpmEnvironment> {
    let Some(value) = arch.get("environment") else {
        return Ok(NpmEnvironment::default());
    };
    let t = value.as_table().ok_or_else(|| {
        anyhow!(
            "architecture.toml `environment` must be a table; got {}",
            value.type_str()
        )
    })?;
    const KNOWN: &[&str] = &["node", "package_manager"];
    for k in t.keys() {
        if !KNOWN.contains(&k.as_str()) {
            return Err(anyhow!(
                "[environment] has unknown key `{k}`; expected one of {}",
                KNOWN.join("/"),
            ));
        }
    }
    let str_key = |key: &str| -> Result<Option<String>> {
        match t.get(key) {
            None => Ok(None),
            Some(toml::Value::String(s)) if s.is_empty() => {
                Err(anyhow!("[environment] `{key}` must be non-empty"))
            }
            Some(toml::Value::String(s)) => Ok(Some(s.clone())),
            Some(other) => Err(anyhow!(
                "[environment] `{key}` must be a string; got {}",
                other.type_str()
            )),
        }
    };
    Ok(NpmEnvironment {
        node: str_key("node")?,
        package_manager: str_key("package_manager")?,
    })
}

/// Read architecture.toml's optional `[scripts]` table: string values
/// only, keyed by script name.
fn extract_scripts(arch: &toml::value::Table) -> Result<BTreeMap<String, String>> {
    let Some(value) = arch.get("scripts") else {
        return Ok(BTreeMap::new());
    };
    let t = value.as_table().ok_or_else(|| {
        anyhow!(
            "architecture.toml `scripts` must be a table; got {}",
            value.type_str()
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in t {
        match v {
            toml::Value::String(s) if !s.is_empty() => {
                out.insert(k.clone(), s.clone());
            }
            toml::Value::String(_) => {
                return Err(anyhow!("[scripts] `{k}` must be non-empty"));
            }
            other => {
                return Err(anyhow!(
                    "[scripts] `{k}` must be a string; got {}",
                    other.type_str()
                ))
            }
        }
    }
    Ok(out)
}

/// Parse one spec-side NpmReference table. Hand-rolled (not serde) so
/// the closed key set is enforced with named errors — a typo'd source
/// key must fail loudly, never silently downgrade to a registry dep.
fn parse_npm_ref(value: &toml::Value) -> Result<NpmRef> {
    let t = value
        .as_table()
        .ok_or_else(|| anyhow!("NpmReference must be a table; got {}", value.type_str()))?;

    const KNOWN: &[&str] = &[
        "package",
        "version",
        "git",
        "path",
        "link",
        "workspace",
        "test_only",
    ];
    for k in t.keys() {
        if !KNOWN.contains(&k.as_str()) {
            return Err(anyhow!(
                "NpmReference has unknown key `{k}`; expected one of {}",
                KNOWN.join("/"),
            ));
        }
    }

    // Strict per-key type checks: a wrong-typed value must error, never
    // silently coerce/default (it would diverge from the schema-side
    // twin, which rejects wrong types).
    let opt_str = |key: &str| -> Result<Option<String>> {
        match t.get(key) {
            None => Ok(None),
            Some(toml::Value::String(s)) if s.is_empty() => {
                Err(anyhow!("NpmReference `{key}` must be non-empty"))
            }
            Some(toml::Value::String(s)) => Ok(Some(s.clone())),
            Some(other) => Err(anyhow!(
                "NpmReference `{key}` must be a string; got {}",
                other.type_str()
            )),
        }
    };

    let package = opt_str("package")?
        .ok_or_else(|| anyhow!("NpmReference missing required `package` (string)"))?;
    let version = opt_str("version")?;
    let test_only = match t.get("test_only") {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(other) => {
            return Err(anyhow!(
                "NpmReference `{package}`: `test_only` must be a boolean; got {}",
                other.type_str()
            ))
        }
    };

    let source_keys: Vec<&str> = ["git", "path", "link", "workspace"]
        .into_iter()
        .filter(|k| t.contains_key(*k))
        .collect();
    if source_keys.len() > 1 {
        return Err(anyhow!(
            "NpmReference `{package}` declares multiple sources ({}); at most \
             one of git/path/link/workspace is allowed",
            source_keys.join(", "),
        ));
    }

    let source = match source_keys.first().copied() {
        None => None,
        Some("path") => Some(NpmSource::Path {
            path: opt_str("path")?
                .ok_or_else(|| anyhow!("NpmReference `{package}`: `path` must be a string"))?,
        }),
        Some("link") => Some(NpmSource::Link {
            link: opt_str("link")?
                .ok_or_else(|| anyhow!("NpmReference `{package}`: `link` must be a string"))?,
        }),
        Some("workspace") => Some(NpmSource::Workspace {
            workspace: opt_str("workspace")?.ok_or_else(|| {
                anyhow!("NpmReference `{package}`: `workspace` must be a range string")
            })?,
        }),
        Some("git") => {
            let g = t
                .get("git")
                .and_then(toml::Value::as_table)
                .ok_or_else(|| {
                    anyhow!(
                        "NpmReference `{package}`: `git` must be a table with `url` \
                     (+ optional ref)"
                    )
                })?;
            for k in g.keys() {
                if !["url", "ref"].contains(&k.as_str()) {
                    return Err(anyhow!(
                        "NpmReference `{package}`: unknown `git` key `{k}`; expected url/ref"
                    ));
                }
            }
            let git_str = |key: &str| -> Result<Option<String>> {
                match g.get(key) {
                    None => Ok(None),
                    Some(toml::Value::String(s)) if s.is_empty() => Err(anyhow!(
                        "NpmReference `{package}`: `git.{key}` must be non-empty"
                    )),
                    Some(toml::Value::String(s)) => Ok(Some(s.clone())),
                    Some(other) => Err(anyhow!(
                        "NpmReference `{package}`: `git.{key}` must be a string; got {}",
                        other.type_str()
                    )),
                }
            };
            Some(NpmSource::Git {
                git: bd_module::NpmGit {
                    url: git_str("url")?.ok_or_else(|| {
                        anyhow!("NpmReference `{package}`: `git.url` is required")
                    })?,
                    r#ref: git_str("ref")?,
                },
            })
        }
        Some(_) => unreachable!("source_keys drawn from the closed list"),
    };

    // A package.json dependency value is one string: a source key owns
    // it, so a version beside a source has no slot to land in.
    if version.is_some() && source.is_some() {
        return Err(anyhow!(
            "NpmReference `{package}` declares `version` beside a source key \
             ({}); a package.json dependency value is one string — put the \
             ref/range inside the source (git.ref = \"semver:^1.0.0\", \
             workspace = \"^1.0.0\")",
            source_keys.join(", "),
        ));
    }
    if version.is_none() && source.is_none() {
        return Err(anyhow!(
            "NpmReference `{package}` is a registry dep but has no `version` \
             range; add one (e.g. version = \"^1.0.0\")"
        ));
    }

    Ok(NpmRef {
        package,
        version,
        source,
        test_only,
    })
}

fn extract_npm_ref_array(value: Option<&toml::Value>) -> Result<Vec<NpmRef>> {
    let Some(v) = value else {
        return Ok(Vec::new());
    };
    let Some(arr) = v.as_array() else {
        return Ok(Vec::new());
    };
    arr.iter().map(parse_npm_ref).collect()
}

fn collect_module_external_deps(layout: &LayoutContext) -> Result<Vec<NpmRef>> {
    let mut out: Vec<NpmRef> = Vec::new();
    if !layout.modules_dir.is_dir() {
        return Ok(out);
    }
    let mut paths: Vec<_> = std::fs::read_dir(&layout.modules_dir)
        .with_context(|| format!("reading {}", layout.modules_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().map(|s| s == "toml").unwrap_or(false))
        .collect();
    paths.sort();
    for path in &paths {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let value: toml::Value =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        for r in extract_npm_ref_array(value.get("external_deps"))
            .with_context(|| format!("in {}", path.display()))?
        {
            out.push(r);
        }
    }
    Ok(out)
}

/// Collect `npm:` contributions from every inlined block or bridge
/// `deps.yaml` (same discovery walk as the cargo and pub paths: unit
/// names from `<inlined dir>/*.schema.yaml`, deps at
/// `<codegen>/<kind>/<unit>/deps.yaml`).
fn collect_inlined_unit_deps(
    layout: &LayoutContext,
    inlined_dir: &std::path::Path,
    kind_dir: &str,
) -> Result<Vec<NpmRef>> {
    let mut out: Vec<NpmRef> = Vec::new();
    if !inlined_dir.is_dir() {
        return Ok(out);
    }
    let mut unit_names: Vec<String> = std::fs::read_dir(inlined_dir)
        .with_context(|| format!("reading {}", inlined_dir.display()))?
        .filter_map(|r| r.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name().and_then(|s| s.to_str())?;
            name.strip_suffix(".schema.yaml").map(String::from)
        })
        .collect();
    unit_names.sort();
    for unit in unit_names {
        let deps_path = layout
            .codegen_dir
            .join(kind_dir)
            .join(&unit)
            .join("deps.yaml");
        let deps = BlockDeps::load(&deps_path).with_context(|| {
            format!(
                "loading {kind_dir} deps.yaml for `{unit}` at {}",
                deps_path.display()
            )
        })?;
        for entry in &deps.npm {
            out.push(NpmRef {
                package: entry.package.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                test_only: false,
            });
        }
    }
    Ok(out)
}

/// Insert `r` into `deps` under the shared merge discipline: first
/// insert wins; exact duplicates are ignored; a version conflict needs
/// both sides to name a version; a source difference warns.
fn merge_dep(deps: &mut BTreeMap<String, NpmRef>, r: NpmRef, warnings: &mut Vec<String>) {
    match deps.get(&r.package) {
        None => {
            deps.insert(r.package.clone(), r);
        }
        Some(existing) if existing == &r => {}
        Some(existing) => {
            if existing.version.is_some() && r.version.is_some() && existing.version != r.version {
                let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "<none>".to_string());
                warnings.push(format!(
                    "package `{}` declared with conflicting versions ({} vs {}); using {}",
                    r.package,
                    show(&existing.version),
                    show(&r.version),
                    show(&existing.version),
                ));
            } else if existing.source != r.source {
                warnings.push(format!(
                    "package `{}` declared with conflicting sources; keeping the \
                     first-declared entry",
                    r.package,
                ));
            }
            // Otherwise the entries agree on everything that can conflict —
            // one side is `test_only` and the other is not. First-declared
            // wins, silently: a package sits in one section.
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// The dependency value string for one reference. Exhaustive over the
/// source shapes — no silent drops.
fn dep_value(r: &NpmRef) -> String {
    match &r.source {
        None => r.version.clone().expect("checked at parse time"),
        Some(NpmSource::Git { git }) => match &git.r#ref {
            Some(re) => format!("{}#{}", git.url, re),
            None => git.url.clone(),
        },
        Some(NpmSource::Path { path }) => format!("file:{path}"),
        Some(NpmSource::Link { link }) => format!("link:{link}"),
        Some(NpmSource::Workspace { workspace }) => format!("workspace:{workspace}"),
    }
}

/// Render the whole document in the fixed key order. Top-level values
/// are pre-rendered JSON fragments; nested maps use a 4-space body.
fn write_package_json(
    name: &str,
    description: Option<&str>,
    header: &Header,
    deps: &BTreeMap<String, NpmRef>,
) -> String {
    let mut fields: Vec<(&str, String)> = Vec::new();
    fields.push(("name", json_str(name)));
    if let Some(v) = &header.version {
        fields.push(("version", json_str(v)));
    }
    if let Some(d) = description {
        fields.push(("description", json_str(d)));
    }
    if header.private {
        fields.push(("private", "true".to_string()));
    }
    if let Some(pm) = &header.package_manager {
        fields.push(("packageManager", json_str(pm)));
    }
    if let Some(t) = header.module_type {
        fields.push(("type", json_str(t)));
    }
    if let Some(node) = &header.engines_node {
        let mut engines = BTreeMap::new();
        engines.insert("node".to_string(), node.clone());
        fields.push(("engines", json_map(&engines)));
    }
    if !header.scripts.is_empty() {
        fields.push(("scripts", json_map(&header.scripts)));
    }
    let regular: BTreeMap<String, String> = deps
        .iter()
        .filter(|(_, r)| !r.test_only)
        .map(|(k, r)| (k.clone(), dep_value(r)))
        .collect();
    if !regular.is_empty() {
        fields.push(("dependencies", json_map(&regular)));
    }
    let dev: BTreeMap<String, String> = deps
        .iter()
        .filter(|(_, r)| r.test_only)
        .map(|(k, r)| (k.clone(), dep_value(r)))
        .collect();
    if !dev.is_empty() {
        fields.push(("devDependencies", json_map(&dev)));
    }

    let mut text = String::from("{\n");
    let n = fields.len();
    for (i, (k, v)) in fields.iter().enumerate() {
        text.push_str(&format!("  {}: {}", json_str(k), v));
        if i + 1 < n {
            text.push(',');
        }
        text.push('\n');
    }
    text.push_str("}\n");
    text
}

/// A lex-sorted string map as a nested JSON object (4-space body,
/// 2-space closing brace). Callers omit empty maps.
fn json_map(map: &BTreeMap<String, String>) -> String {
    let mut out = String::from("{\n");
    let n = map.len();
    for (i, (k, v)) in map.iter().enumerate() {
        out.push_str(&format!("    {}: {}", json_str(k), json_str(v)));
        if i + 1 < n {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("  }");
    out
}

/// JSON string literal — serde_json's escaping, so the emitted bytes are
/// exactly what a JSON parser round-trips.
fn json_str(s: &str) -> String {
    serde_json::to_string(s).expect("a &str always serializes")
}

/// Double-quote a YAML scalar for `pnpm-workspace.yaml` (member paths).
fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Legal npm package name: at most 214 characters, lowercase, URL-safe
/// (`[a-z0-9-._~]`), no leading `.` or `_`, optional `@scope/` prefix
/// whose scope follows the same segment rule. Warn-only — the project is
/// renamed deliberately, never rewritten.
fn is_legal_npm_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 214 {
        return false;
    }
    let segment_ok = |seg: &str| {
        !seg.is_empty()
            && !seg.starts_with('.')
            && !seg.starts_with('_')
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._~".contains(c))
    };
    match name.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, pkg)) => segment_ok(scope) && segment_ok(pkg),
            None => false,
        },
        None => segment_ok(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toml_value(s: &str) -> toml::Value {
        toml::from_str(s).unwrap()
    }

    fn header(private: bool) -> Header {
        Header {
            version: Some("0.1.0".into()),
            private,
            package_manager: Some("pnpm@10.15.0".into()),
            module_type: Some("module"),
            engines_node: Some(">=22.18.0".into()),
            scripts: BTreeMap::from([("check".to_string(), "tsc --noEmit".to_string())]),
        }
    }

    #[test]
    fn parse_npm_ref_registry_requires_version() {
        let ok = parse_npm_ref(&toml_value("package = \"next\"\nversion = \"^16.3.0\"")).unwrap();
        assert_eq!(ok.package, "next");
        assert!(ok.source.is_none());

        let err = parse_npm_ref(&toml_value("package = \"next\"")).unwrap_err();
        assert!(format!("{err:#}").contains("no `version`"), "{err:#}");
    }

    #[test]
    fn parse_npm_ref_rejects_unknown_multi_source_and_version_beside_source() {
        let err = parse_npm_ref(&toml_value("package = \"x\"\npth = \"../x\"")).unwrap_err();
        assert!(format!("{err:#}").contains("unknown key `pth`"), "{err:#}");

        let err = parse_npm_ref(&toml_value(
            "package = \"x\"\npath = \"../x\"\nlink = \"../x\"",
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("multiple sources"), "{err:#}");

        let err = parse_npm_ref(&toml_value(
            "package = \"x\"\npath = \"../x\"\nversion = \"^1.0.0\"",
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("`version` beside a source key"),
            "{err:#}"
        );
    }

    #[test]
    fn dep_values_cover_every_source_shape() {
        let cases: &[(&str, &str)] = &[
            ("package = \"next\"\nversion = \"^16.3.0\"", "^16.3.0"),
            (
                "package = \"g\"\ngit = { url = \"git+https://g.example/g.git\", ref = \"v1.4.0\" }",
                "git+https://g.example/g.git#v1.4.0",
            ),
            (
                "package = \"g2\"\ngit = { url = \"github:acme/g2\" }",
                "github:acme/g2",
            ),
            ("package = \"sib\"\npath = \"../sib\"", "file:../sib"),
            ("package = \"sib\"\nlink = \"../sib\"", "link:../sib"),
            ("package = \"shared\"\nworkspace = \"*\"", "workspace:*"),
        ];
        for (src, expected) in cases {
            let r = parse_npm_ref(&toml_value(src)).unwrap();
            assert_eq!(dep_value(&r), *expected, "for {src}");
        }
    }

    #[test]
    fn merge_keeps_first_and_warns_on_version_and_source_conflicts() {
        let mut deps = BTreeMap::new();
        let mut warnings = Vec::new();
        let a = parse_npm_ref(&toml_value("package = \"react\"\nversion = \"^19.2.0\"")).unwrap();
        let b = parse_npm_ref(&toml_value("package = \"react\"\nversion = \"^18.0.0\"")).unwrap();
        merge_dep(&mut deps, a, &mut warnings);
        merge_dep(&mut deps, b, &mut warnings);
        assert_eq!(deps["react"].version.as_deref(), Some("^19.2.0"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("conflicting versions"));

        // Registry vs link: no version on one side, so not a version
        // conflict — a source conflict.
        let c = parse_npm_ref(&toml_value("package = \"react\"\nlink = \"../react\"")).unwrap();
        merge_dep(&mut deps, c, &mut warnings);
        assert_eq!(warnings.len(), 2);
        assert!(warnings[1].contains("conflicting sources"));
        assert!(deps["react"].source.is_none(), "first-declared source wins");
    }

    #[test]
    fn document_key_order_is_fixed_and_empty_maps_are_omitted() {
        let mut deps = BTreeMap::new();
        for src in [
            "package = \"zod\"\nversion = \"^4.0.0\"",
            "package = \"@types/node\"\nversion = \"^22.0.0\"\ntest_only = true",
            "package = \"next\"\nversion = \"^16.3.0\"",
        ] {
            let r = parse_npm_ref(&toml_value(src)).unwrap();
            deps.insert(r.package.clone(), r);
        }
        let text = write_package_json("demo", Some("A demo."), &header(true), &deps);
        let expected = r#"{
  "name": "demo",
  "version": "0.1.0",
  "description": "A demo.",
  "private": true,
  "packageManager": "pnpm@10.15.0",
  "type": "module",
  "engines": {
    "node": ">=22.18.0"
  },
  "scripts": {
    "check": "tsc --noEmit"
  },
  "dependencies": {
    "next": "^16.3.0",
    "zod": "^4.0.0"
  },
  "devDependencies": {
    "@types/node": "^22.0.0"
  }
}
"#;
        assert_eq!(text, expected);
        // Parses as JSON and keeps the key order.
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["private"], serde_json::Value::Bool(true));

        let bare = Header {
            version: None,
            private: false,
            package_manager: None,
            module_type: None,
            engines_node: None,
            scripts: BTreeMap::new(),
        };
        let text = write_package_json("lib", None, &bare, &BTreeMap::new());
        assert_eq!(text, "{\n  \"name\": \"lib\"\n}\n");
    }

    #[test]
    fn npm_name_legality() {
        assert!(is_legal_npm_name("ble-scan"));
        assert!(is_legal_npm_name("@acme/ble-scan"));
        assert!(is_legal_npm_name("ble_scan"));
        assert!(!is_legal_npm_name("BleScan"), "uppercase is illegal");
        assert!(
            !is_legal_npm_name("_private"),
            "leading underscore is illegal"
        );
        assert!(!is_legal_npm_name(".hidden"), "leading dot is illegal");
        assert!(
            !is_legal_npm_name("@acme"),
            "scope without a name is illegal"
        );
        assert!(!is_legal_npm_name(""), "empty is illegal");
        assert!(
            !is_legal_npm_name(&"a".repeat(215)),
            "over 214 chars is illegal"
        );
    }

    #[test]
    fn environment_and_scripts_reject_unknown_keys_and_wrong_types() {
        let arch = toml_value("[environment]\nnode = \">=22\"\nnpm = \"10\"");
        let err = extract_environment(arch.as_table().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("unknown key `npm`"), "{err:#}");

        let arch = toml_value("[scripts]\ndev = 3");
        let err = extract_scripts(arch.as_table().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("must be a string"), "{err:#}");

        let arch = toml_value("version = 45");
        let err = extract_app_version(arch.as_table().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("must be a string"), "{err:#}");
    }

    /// The embedded NpmReference schema and this module's hand-rolled
    /// parser describe the same grammar in two places. This test locks
    /// them together: for every case, the schema verdict and the parser
    /// verdict must agree (and match the expected verdict). A divergence
    /// means validate-passes-but-emit-fails (or the reverse) shipped.
    #[test]
    fn npm_reference_schema_and_parser_agree() {
        let schema = crate::embedded_schemas::embedded_schema_entries()
            .iter()
            .find(|(keys, _)| keys.iter().any(|k| k == "npm_reference.schema.yaml"))
            .expect("embedded NpmReference schema")
            .1
            .pointer("/$defs/NpmReference")
            .expect("$defs/NpmReference present")
            .clone();
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");

        // (toml source, expected-accepted)
        let cases: &[(&str, bool)] = &[
            ("package = \"next\"\nversion = \"^16.3.0\"", true),
            (
                "package = \"@types/node\"\nversion = \"^22.0.0\"\ntest_only = true",
                true,
            ),
            (
                "package = \"g\"\ngit = { url = \"git+https://g.example/g.git\", ref = \"v1\" }",
                true,
            ),
            ("package = \"g\"\ngit = { url = \"github:acme/g\" }", true),
            ("package = \"sib\"\npath = \"../sib\"", true),
            ("package = \"sib\"\nlink = \"../sib\"", true),
            ("package = \"shared\"\nworkspace = \"*\"", true),
            // Rejections.
            ("package = \"next\"", false),
            (
                "package = \"sib\"\npath = \"../sib\"\nversion = \"^1.0.0\"",
                false,
            ),
            (
                "package = \"g\"\ngit = { url = \"u\" }\nversion = \"^1.0.0\"",
                false,
            ),
            (
                "package = \"shared\"\nworkspace = \"*\"\nversion = \"^1.0.0\"",
                false,
            ),
            ("package = \"x\"\npth = \"../x\"", false),
            ("package = \"x\"\npath = \"../x\"\nlink = \"../x\"", false),
            ("package = \"x\"\npath = \"../x\"\nworkspace = \"*\"", false),
            ("package = \"g\"\ngit = { ref = \"v1\" }", false),
            (
                "package = \"g\"\ngit = { url = \"u\", rev = \"abc\" }",
                false,
            ),
            ("package = \"g\"\ngit = { url = \"u\", ref = 1 }", false),
            ("package = \"next\"\nversion = 16", false),
            ("package = \"next\"\nversion = \"\"", false),
            (
                "package = \"next\"\nversion = \"^16.0.0\"\ntest_only = \"yes\"",
                false,
            ),
            ("package = \"\"\nversion = \"^1.0.0\"", false),
            ("package = \"sib\"\npath = \"\"", false),
        ];

        for (src, expected_ok) in cases {
            let value = toml_value(src);
            let json = serde_json::to_value(&value).expect("toml → json");
            let schema_ok = validator.validate(&json).is_ok();
            let parser_ok = parse_npm_ref(&value).is_ok();
            assert_eq!(
                schema_ok, *expected_ok,
                "schema verdict diverges for: {src}"
            );
            assert_eq!(
                parser_ok, *expected_ok,
                "parser verdict diverges for: {src}"
            );
        }
    }
}
