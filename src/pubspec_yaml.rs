//! Deterministic pubspec.yaml emission from spec metadata.
//!
//! The Dart sibling of `cargo_toml.rs` (deliberately a concrete sibling
//! emitter, not a shared rendering abstraction — the two share merge
//! *discipline*, not code; revisit if a third language emitter lands).
//! Reads `spec.toml` `[project]` + `architecture.toml` (`[environment]`,
//! `[[external_deps]]`, `[[dependency_overrides]]`) + module-level
//! `external_deps[]` + inlined block/bridge `deps.yaml` `pub:`
//! contributions, and emits a pubspec.yaml whose content is a
//! deterministic projection of the spec.
//!
//! Every pub dep is in the spec; pubspec.yaml is OUTPUT — same shape as
//! the Cargo path. Merge order: user (architecture + modules) > blocks >
//! bridges; first insert wins, conflicts warn.
//!
//! architecture.toml also carries `version` (emitted as the pubspec
//! `version:` field) and the `[flutter]` table (`uses_material_design`
//! → `uses-material-design`, `assets` → `flutter: assets:`); both are
//! optional and package-shape only (the workspace root is a pure
//! aggregator). Custom font declarations are not modeled — named, not
//! silently dropped: an unknown `[flutter]` key is a hard error.

use std::collections::BTreeMap;

use anyhow::{anyhow, Context, Result};

use crate::layout::LayoutContext;
use crate::manifest::block_deps as bd_module;
use crate::manifest::{BlockDeps, Profile, PubShapeKind, PubSource};

/// One pubspec dependency entry, normalized. Spec-side twin of the
/// manifest loader's `PubEntry` (which lacks `test_only` — block/bridge
/// contributions are runtime deps by rule).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PubRef {
    package: String,
    version: Option<String>,
    source: Option<PubSource>,
    test_only: bool,
}

/// Result of a pubspec.yaml render: the rendered text plus any
/// non-fatal warnings (e.g., version conflicts across contributors).
#[derive(Debug, Clone)]
pub struct RenderedPubspec {
    pub text: String,
    pub warnings: Vec<String>,
}

/// Render pubspec.yaml for the project at `layout`. Dispatches across
/// three cases, mirroring the cargo path:
///
/// * **Workspace root** (`[workspace]` table set in spec.toml) → emits
///   the root pubspec with a `workspace:` member-path list.
/// * **Workspace member** (an ancestor spec.toml's `[workspace]`
///   includes this directory) → per-shape body plus
///   `resolution: workspace`.
/// * **Standalone** → per-shape body from the inlined profile.yaml's
///   `pubspec_yaml` section. The section is required — a Dart project
///   without it is a configuration error, not a fallback case.
pub fn render(layout: &LayoutContext) -> Result<RenderedPubspec> {
    let manifest = load_inlined_profile(layout)?;
    let pubspec = manifest.pubspec_yaml.as_ref().ok_or_else(|| {
        anyhow!(
            "profile.yaml at {} has no `pubspec_yaml` section; cannot dispatch \
             pubspec render (the profile must declare shape + environment for \
             its language)",
            layout.specdev_root.join("profile.yaml").display(),
        )
    })?;

    if layout.spec.workspace.is_some() {
        return render_workspace_root(layout, pubspec);
    }
    let is_member = has_workspace_ancestor(layout)?;
    render_package(layout, pubspec, is_member)
}

/// True when an ancestor spec.toml carries a `[workspace]` whose
/// `members[].path` includes this project's directory. Deliberately NOT
/// the cargo path's `WorkspaceContext` — that walker also parses the
/// workspace's `[[workspace_dependencies]]` as *cargo* refs, which
/// would fail with cargo-vocabulary errors on a Dart workspace. Pub
/// member mode needs only the membership boolean.
fn has_workspace_ancestor(layout: &LayoutContext) -> Result<bool> {
    let member_dir = layout.project_root.as_path();
    let mut cursor = match member_dir.parent() {
        Some(p) => p,
        None => return Ok(false),
    };
    loop {
        let candidate = cursor.join("spec.toml");
        if candidate.is_file() {
            let text = std::fs::read_to_string(&candidate)
                .with_context(|| format!("reading {}", candidate.display()))?;
            if let Ok(parsed) = toml::from_str::<toml::Value>(&text) {
                let members = parsed
                    .get("workspace")
                    .and_then(|w| w.get("members"))
                    .and_then(|m| m.as_array());
                if let Some(members) = members {
                    let root = candidate.parent().unwrap_or(&candidate);
                    if let Ok(rel) = member_dir.strip_prefix(root) {
                        let rel = rel.to_string_lossy().replace('\\', "/");
                        if members.iter().any(|m| {
                            m.get("path")
                                .and_then(|p| p.as_str())
                                .map(|p| p.trim_end_matches('/') == rel)
                                .unwrap_or(false)
                        }) {
                            return Ok(true);
                        }
                    }
                }
            }
        }
        match cursor.parent() {
            Some(p) => cursor = p,
            None => return Ok(false),
        }
    }
}

fn load_inlined_profile(layout: &LayoutContext) -> Result<Profile> {
    let path = layout.specdev_root.join("profile.yaml");
    if !path.is_file() {
        return Err(anyhow!(
            "no profile.yaml at {} — pubspec emission is manifest-driven and \
             has no pre-manifest fallback; re-bootstrap or run `specdev migrate`",
            path.display(),
        ));
    }
    Profile::load(&path)
}

/// Resolve the `environment:` block: architecture.toml's `[environment]`
/// table (keys `sdk`, `flutter`) overrides the profile manifest's
/// `pubspec_yaml.environment` defaults, key by key. An sdk constraint
/// must come from one of the two — pubspec requires it.
fn resolve_environment(
    layout: &LayoutContext,
    pubspec: &crate::manifest::PubspecYamlSection,
    arch: &toml::value::Table,
) -> Result<(String, Option<String>)> {
    let arch_env = arch.get("environment").and_then(toml::Value::as_table);
    let arch_sdk = arch_env
        .and_then(|t| t.get("sdk"))
        .and_then(toml::Value::as_str)
        .map(String::from);
    let arch_flutter = arch_env
        .and_then(|t| t.get("flutter"))
        .and_then(toml::Value::as_str)
        .map(String::from);

    let sdk = arch_sdk
        .or_else(|| pubspec.environment.as_ref().map(|e| e.sdk.clone()))
        .ok_or_else(|| {
            anyhow!(
                "no Dart SDK constraint available: add `[environment] sdk = \"...\"` \
                 to {} or an `environment.sdk` default to the profile's \
                 pubspec_yaml section",
                layout.architecture_toml_path.display(),
            )
        })?;
    let flutter = arch_flutter.or_else(|| {
        pubspec
            .environment
            .as_ref()
            .and_then(|e| e.flutter.clone())
    });
    Ok((sdk, flutter))
}

fn render_package(
    layout: &LayoutContext,
    pubspec: &crate::manifest::PubspecYamlSection,
    workspace_member: bool,
) -> Result<RenderedPubspec> {
    let project = &layout.spec.project;
    let arch = read_architecture(layout)?;
    let (sdk, flutter) = resolve_environment(layout, pubspec, &arch)?;

    let mut deps: BTreeMap<String, PubRef> = BTreeMap::new();
    let mut warnings: Vec<String> = Vec::new();

    // User-authored deps first (their versions win on conflict):
    // architecture-level [[external_deps]], then per-module external_deps[].
    for r in extract_pub_ref_array(arch.get("external_deps"))? {
        merge_dep(&mut deps, r, &mut warnings);
    }
    for r in collect_module_external_deps(layout)? {
        merge_dep(&mut deps, r, &mut warnings);
    }
    // Block-contributed deps, then bridge join-product deps
    // (user > blocks > bridges; first insert wins, conflicts warn).
    for r in collect_inlined_unit_deps(layout, &layout.inlined_blocks_dir, "blocks")? {
        merge_dep(&mut deps, r, &mut warnings);
    }
    for r in collect_inlined_unit_deps(layout, &layout.inlined_bridges_dir, "bridges")? {
        merge_dep(&mut deps, r, &mut warnings);
    }

    let name = pub_package_name(&project.name);
    if !is_legal_pub_name(&name) {
        warnings.push(format!(
            "package name `{name}` (from [project].name) is not a legal pub \
             identifier (lowercase_with_underscores, not starting with a \
             digit); pub will reject the emitted pubspec until the project \
             is renamed",
        ));
    }

    let version = extract_app_version(&arch)?;
    let flutter_config = extract_flutter_config(&arch)?;

    let mut text = String::new();
    write_header(&mut text, project, version.as_deref(), matches!(pubspec.shape, PubShapeKind::App));
    write_environment(&mut text, &sdk, flutter.as_deref());
    if workspace_member {
        text.push_str("resolution: workspace\n");
    }
    write_dependency_sections(&mut text, &deps);
    write_overrides_section(&mut text, &arch)?;
    write_flutter_section(&mut text, &flutter_config);

    Ok(RenderedPubspec { text, warnings })
}

/// Render the workspace-root pubspec: `workspace:` member-path list
/// (declaration order, matching the cargo members convention). The root
/// is a pure aggregator in this model — pub's documented placeholder
/// `name: _` (never published, never a package of its own); the
/// members are the packages.
fn render_workspace_root(
    layout: &LayoutContext,
    pubspec: &crate::manifest::PubspecYamlSection,
) -> Result<RenderedPubspec> {
    let workspace_section = layout
        .spec
        .workspace
        .as_ref()
        .expect("render_workspace_root only called when [workspace] is set");
    let arch = read_architecture(layout)?;
    let (sdk, flutter) = resolve_environment(layout, pubspec, &arch)?;

    let mut text = String::new();
    text.push_str("name: _\n");
    text.push_str("publish_to: none\n");
    write_environment(&mut text, &sdk, flutter.as_deref());
    text.push_str("workspace:\n");
    for member in &workspace_section.members {
        text.push_str(&format!("  - {}\n", quote_str(&member.path)));
    }

    Ok(RenderedPubspec {
        text,
        warnings: Vec::new(),
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
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading {}", path.display()))?;
    let value: toml::Value = toml::from_str(&text)
        .with_context(|| format!("parsing {}", path.display()))?;
    match value {
        toml::Value::Table(t) => Ok(t),
        _ => Err(anyhow!("{} did not parse as a TOML table", path.display())),
    }
}

/// The architecture `[flutter]` table, hand-parsed with a closed key
/// set (same discipline as PubReference — an unknown key must fail
/// loudly, never silently drop; custom fonts are deliberately not
/// modeled yet and fail as unknown keys).
#[derive(Debug, Clone, Default)]
struct FlutterConfig {
    uses_material_design: Option<bool>,
    assets: Vec<String>,
}

impl FlutterConfig {
    fn is_empty(&self) -> bool {
        self.uses_material_design.is_none() && self.assets.is_empty()
    }
}

/// Read architecture.toml's optional top-level `version` (the pubspec
/// `version:` field — semver plus optional `+build`). Type errors are
/// hard failures; absence is fine.
fn extract_app_version(arch: &toml::value::Table) -> Result<Option<String>> {
    match arch.get("version") {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(anyhow!(
            "architecture.toml `version` must be a string (e.g. \"1.2.3+45\"); \
             got {}",
            other.type_str(),
        )),
    }
}

/// Read architecture.toml's optional `[flutter]` table.
fn extract_flutter_config(arch: &toml::value::Table) -> Result<FlutterConfig> {
    let Some(value) = arch.get("flutter") else {
        return Ok(FlutterConfig::default());
    };
    let t = value.as_table().ok_or_else(|| {
        anyhow!(
            "architecture.toml `flutter` must be a table; got {}",
            value.type_str()
        )
    })?;
    const KNOWN: &[&str] = &["uses_material_design", "assets"];
    for k in t.keys() {
        if !KNOWN.contains(&k.as_str()) {
            return Err(anyhow!(
                "[flutter] has unknown key `{k}`; expected one of {}",
                KNOWN.join("/"),
            ));
        }
    }
    let uses_material_design = match t.get("uses_material_design") {
        None => None,
        Some(toml::Value::Boolean(b)) => Some(*b),
        Some(other) => {
            return Err(anyhow!(
                "[flutter] `uses_material_design` must be a boolean; got {}",
                other.type_str(),
            ))
        }
    };
    let assets = match t.get("assets") {
        None => Vec::new(),
        Some(toml::Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str().map(String::from).ok_or_else(|| {
                    anyhow!(
                        "[flutter] `assets` entries must be strings; got {}",
                        v.type_str()
                    )
                })
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => {
            return Err(anyhow!(
                "[flutter] `assets` must be an array of strings; got {}",
                other.type_str(),
            ))
        }
    };
    Ok(FlutterConfig {
        uses_material_design,
        assets,
    })
}

/// Parse one spec-side PubReference table. Hand-rolled (not serde) so
/// the closed key set is enforced with named errors — a typo'd source
/// key must fail loudly, never silently downgrade to a registry dep
/// (the pass-through-silent-drop lesson).
fn parse_pub_ref(value: &toml::Value) -> Result<PubRef> {
    let t = value
        .as_table()
        .ok_or_else(|| anyhow!("PubReference must be a table; got {}", value.type_str()))?;

    const KNOWN: &[&str] = &[
        "package", "version", "hosted", "git", "path", "sdk", "test_only",
    ];
    for k in t.keys() {
        if !KNOWN.contains(&k.as_str()) {
            return Err(anyhow!(
                "PubReference has unknown key `{k}`; expected one of {}",
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
                Err(anyhow!("PubReference `{key}` must be non-empty"))
            }
            Some(toml::Value::String(s)) => Ok(Some(s.clone())),
            Some(other) => Err(anyhow!(
                "PubReference `{key}` must be a string; got {}",
                other.type_str()
            )),
        }
    };

    let package = opt_str("package")?
        .ok_or_else(|| anyhow!("PubReference missing required `package` (string)"))?;
    let version = opt_str("version")?;
    let test_only = match t.get("test_only") {
        None => false,
        Some(toml::Value::Boolean(b)) => *b,
        Some(other) => {
            return Err(anyhow!(
                "PubReference `{package}`: `test_only` must be a boolean; got {}",
                other.type_str()
            ))
        }
    };

    let source_keys: Vec<&str> = ["hosted", "git", "path", "sdk"]
        .into_iter()
        .filter(|k| t.contains_key(*k))
        .collect();
    if source_keys.len() > 1 {
        return Err(anyhow!(
            "PubReference `{package}` declares multiple sources ({}); at most \
             one of hosted/git/path/sdk is allowed",
            source_keys.join(", "),
        ));
    }

    let source = match source_keys.first().copied() {
        None => None,
        Some("hosted") => Some(PubSource::Hosted {
            hosted: opt_str("hosted")?
                .ok_or_else(|| anyhow!("PubReference `{package}`: `hosted` must be a URL string"))?,
        }),
        Some("path") => Some(PubSource::Path {
            path: opt_str("path")?
                .ok_or_else(|| anyhow!("PubReference `{package}`: `path` must be a string"))?,
        }),
        Some("sdk") => Some(PubSource::Sdk {
            sdk: opt_str("sdk")?
                .ok_or_else(|| anyhow!("PubReference `{package}`: `sdk` must be a string"))?,
        }),
        Some("git") => {
            let g = t.get("git").and_then(toml::Value::as_table).ok_or_else(|| {
                anyhow!(
                    "PubReference `{package}`: `git` must be a table with `url` \
                     (+ optional ref/path/tag_pattern)"
                )
            })?;
            for k in g.keys() {
                if !["url", "ref", "path", "tag_pattern"].contains(&k.as_str()) {
                    return Err(anyhow!(
                        "PubReference `{package}`: unknown `git` key `{k}`; expected \
                         url/ref/path/tag_pattern"
                    ));
                }
            }
            let git_str = |key: &str| -> Result<Option<String>> {
                match g.get(key) {
                    None => Ok(None),
                    Some(toml::Value::String(s)) if s.is_empty() => Err(anyhow!(
                        "PubReference `{package}`: `git.{key}` must be non-empty"
                    )),
                    Some(toml::Value::String(s)) => Ok(Some(s.clone())),
                    Some(other) => Err(anyhow!(
                        "PubReference `{package}`: `git.{key}` must be a string; got {}",
                        other.type_str()
                    )),
                }
            };
            Some(PubSource::Git {
                git: bd_module::PubGit {
                    url: git_str("url")?
                        .ok_or_else(|| anyhow!("PubReference `{package}`: `git.url` is required"))?,
                    r#ref: git_str("ref")?,
                    path: git_str("path")?,
                    tag_pattern: git_str("tag_pattern")?,
                },
            })
        }
        Some(_) => unreachable!("source_keys drawn from the closed list"),
    };

    // Registry-hosted deps (default or custom URL) need a version. The
    // other sources may still carry one (pub checks it against the
    // resolved package; with `git.tag_pattern` it feeds the version
    // solver), so it stays optional there — and is always emitted when
    // present.
    if version.is_none() && matches!(source, None | Some(PubSource::Hosted { .. })) {
        return Err(anyhow!(
            "PubReference `{package}` is registry-hosted but has no `version` \
             constraint; add one (e.g. version = \"^1.0.0\")"
        ));
    }

    Ok(PubRef {
        package,
        version,
        source,
        test_only,
    })
}

fn extract_pub_ref_array(value: Option<&toml::Value>) -> Result<Vec<PubRef>> {
    let Some(v) = value else { return Ok(Vec::new()) };
    let Some(arr) = v.as_array() else {
        return Ok(Vec::new());
    };
    arr.iter().map(parse_pub_ref).collect()
}

fn collect_module_external_deps(layout: &LayoutContext) -> Result<Vec<PubRef>> {
    let mut out: Vec<PubRef> = Vec::new();
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
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let value: toml::Value = toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        for r in extract_pub_ref_array(value.get("external_deps"))
            .with_context(|| format!("in {}", path.display()))?
        {
            out.push(r);
        }
    }
    Ok(out)
}

/// Collect `pub:` contributions from every inlined block or bridge
/// `deps.yaml` (same discovery walk as the cargo path: unit names from
/// `<inlined dir>/*.schema.yaml`, deps at `<codegen>/<kind>/<unit>/deps.yaml`).
fn collect_inlined_unit_deps(
    layout: &LayoutContext,
    inlined_dir: &std::path::Path,
    kind_dir: &str,
) -> Result<Vec<PubRef>> {
    let mut out: Vec<PubRef> = Vec::new();
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
        for entry in &deps.pub_packages {
            out.push(PubRef {
                package: entry.package.clone(),
                version: entry.version.clone(),
                source: entry.source.clone(),
                test_only: false,
            });
        }
    }
    Ok(out)
}

/// Insert `r` into `deps` under the cargo-path merge discipline: first
/// insert wins; exact duplicates are ignored; version or source
/// conflicts warn and keep the existing entry.
fn merge_dep(deps: &mut BTreeMap<String, PubRef>, r: PubRef, warnings: &mut Vec<String>) {
    match deps.get(&r.package) {
        None => {
            deps.insert(r.package.clone(), r);
        }
        Some(existing) if existing == &r => {}
        Some(existing) if existing.version != r.version => {
            let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "<none>".to_string());
            warnings.push(format!(
                "package `{}` declared with conflicting versions ({} vs {}); using {}",
                r.package,
                show(&existing.version),
                show(&r.version),
                show(&existing.version),
            ));
        }
        Some(_) => {
            warnings.push(format!(
                "package `{}` declared with conflicting sources; keeping the \
                 first-declared entry",
                r.package,
            ));
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn write_header(
    text: &mut String,
    project: &crate::config::ProjectSection,
    version: Option<&str>,
    publish_to_none: bool,
) {
    text.push_str(&format!("name: {}\n", pub_package_name(&project.name)));
    if let Some(desc) = &project.description {
        text.push_str(&format!("description: {}\n", quote_str(desc)));
    }
    if let Some(v) = version {
        text.push_str(&format!("version: {}\n", quote_str(v)));
    }
    if publish_to_none {
        text.push_str("publish_to: none\n");
    }
}

fn write_environment(text: &mut String, sdk: &str, flutter: Option<&str>) {
    text.push_str("environment:\n");
    text.push_str(&format!("  sdk: {}\n", quote_str(sdk)));
    if let Some(f) = flutter {
        text.push_str(&format!("  flutter: {}\n", quote_str(f)));
    }
}

fn write_dependency_sections(text: &mut String, deps: &BTreeMap<String, PubRef>) {
    // A section header with zero entries would render as a YAML null
    // (`dependencies:` followed by nothing) — not the empty map pub
    // expects — so empty sections are omitted entirely.
    let regular: Vec<(&String, &PubRef)> = deps.iter().filter(|(_, r)| !r.test_only).collect();
    if !regular.is_empty() {
        text.push_str("dependencies:\n");
        for (name, r) in regular {
            write_dep_entry(text, name, r);
        }
    }
    let dev: Vec<(&String, &PubRef)> = deps.iter().filter(|(_, r)| r.test_only).collect();
    if !dev.is_empty() {
        text.push_str("dev_dependencies:\n");
        for (name, r) in dev {
            write_dep_entry(text, name, r);
        }
    }
}

fn write_overrides_section(text: &mut String, arch: &toml::value::Table) -> Result<()> {
    let overrides = extract_pub_ref_array(arch.get("dependency_overrides"))?;
    if overrides.is_empty() {
        return Ok(());
    }
    let mut sorted: BTreeMap<String, PubRef> = BTreeMap::new();
    for r in overrides {
        sorted.insert(r.package.clone(), r);
    }
    text.push_str("dependency_overrides:\n");
    for (name, r) in &sorted {
        write_dep_entry(text, name, r);
    }
    Ok(())
}

/// Render one dep entry (two-space indented under its section). Source
/// dispatch is exhaustive, and a `version` constraint is emitted for
/// EVERY shape it appears on — pub accepts `version:` alongside any
/// source (with `git.tag_pattern` it is what the solver resolves
/// against), so dropping it would silently change resolution:
///   * no source        → `name: "<version>"` (default registry)
///   * hosted           → nested `hosted:` URL + `version:`
///   * git              → nested `git:` block + sibling `version:`
///   * path             → nested `path:` + sibling `version:`
///   * sdk              → nested `sdk:` + sibling `version:`
fn write_dep_entry(text: &mut String, name: &str, r: &PubRef) {
    let sibling_version = |text: &mut String| {
        if let Some(v) = &r.version {
            text.push_str(&format!("    version: {}\n", quote_str(v)));
        }
    };
    match &r.source {
        None => {
            let v = r.version.as_deref().expect("checked at parse time");
            text.push_str(&format!("  {}: {}\n", name, quote_str(v)));
        }
        Some(PubSource::Hosted { hosted }) => {
            text.push_str(&format!("  {}:\n", name));
            text.push_str(&format!("    hosted: {}\n", quote_str(hosted)));
            sibling_version(text);
        }
        Some(PubSource::Git { git }) => {
            text.push_str(&format!("  {}:\n", name));
            text.push_str("    git:\n");
            text.push_str(&format!("      url: {}\n", quote_str(&git.url)));
            if let Some(re) = &git.r#ref {
                text.push_str(&format!("      ref: {}\n", quote_str(re)));
            }
            if let Some(p) = &git.path {
                text.push_str(&format!("      path: {}\n", quote_str(p)));
            }
            if let Some(tp) = &git.tag_pattern {
                text.push_str(&format!("      tag_pattern: {}\n", quote_str(tp)));
            }
            sibling_version(text);
        }
        Some(PubSource::Path { path }) => {
            text.push_str(&format!("  {}:\n", name));
            text.push_str(&format!("    path: {}\n", quote_str(path)));
            sibling_version(text);
        }
        Some(PubSource::Sdk { sdk }) => {
            text.push_str(&format!("  {}:\n", name));
            text.push_str(&format!("    sdk: {}\n", sdk));
            sibling_version(text);
        }
    }
}

/// Render the pubspec `flutter:` config block (last section by
/// convention). An empty config emits nothing — a bare `flutter:`
/// header would be YAML null, not the empty map pub expects.
fn write_flutter_section(text: &mut String, config: &FlutterConfig) {
    if config.is_empty() {
        return;
    }
    text.push_str("flutter:\n");
    if let Some(b) = config.uses_material_design {
        text.push_str(&format!("  uses-material-design: {b}\n"));
    }
    if !config.assets.is_empty() {
        text.push_str("  assets:\n");
        for a in &config.assets {
            text.push_str(&format!("    - {}\n", quote_str(a)));
        }
    }
}

/// A project name as a pub package identifier: pub requires
/// lower_snake_case, so hyphens become underscores (the analog of the
/// cargo path's `crate_ident`). Other illegalities (uppercase, leading
/// digit) are not silently rewritten — the renderer warns instead, so
/// the author renames deliberately.
fn pub_package_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Legal pub package identifier: `[a-z_][a-z0-9_]*`.
fn is_legal_pub_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Double-quote a YAML scalar, escaping the double-quoted-style special
/// characters. Version constraints (`^1.2.0`, `>=2.0.0 <3.0.0`) and
/// URLs are always emitted quoted — uniformly valid regardless of YAML
/// plain-scalar edge cases.
fn quote_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toml_value(s: &str) -> toml::Value {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn parse_pub_ref_hosted_default_requires_version() {
        let ok = parse_pub_ref(&toml_value("package = \"http\"\nversion = \"^1.2.0\"")).unwrap();
        assert_eq!(ok.package, "http");
        assert!(ok.source.is_none());

        let err = parse_pub_ref(&toml_value("package = \"http\"")).unwrap_err();
        assert!(format!("{err:#}").contains("no `version`"), "{err:#}");
    }

    #[test]
    fn parse_pub_ref_rejects_unknown_and_multi_source_keys() {
        let err = parse_pub_ref(&toml_value("package = \"x\"\npth = \"../x\"")).unwrap_err();
        assert!(format!("{err:#}").contains("unknown key `pth`"), "{err:#}");

        let err = parse_pub_ref(&toml_value(
            "package = \"x\"\npath = \"../x\"\nsdk = \"flutter\"",
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("multiple sources"), "{err:#}");
    }

    #[test]
    fn parse_pub_ref_source_shapes() {
        let sdk = parse_pub_ref(&toml_value("package = \"flutter\"\nsdk = \"flutter\"")).unwrap();
        assert!(matches!(sdk.source, Some(PubSource::Sdk { .. })));
        assert!(sdk.version.is_none());

        let path = parse_pub_ref(&toml_value("package = \"local\"\npath = \"../local\"")).unwrap();
        assert!(matches!(path.source, Some(PubSource::Path { .. })));

        let git = parse_pub_ref(&toml_value(
            "package = \"foo\"\ngit = { url = \"https://g.example/foo\", ref = \"v1\" }",
        ))
        .unwrap();
        match git.source {
            Some(PubSource::Git { git }) => {
                assert_eq!(git.url, "https://g.example/foo");
                assert_eq!(git.r#ref.as_deref(), Some("v1"));
            }
            other => panic!("expected git source, got {other:?}"),
        }

        let err = parse_pub_ref(&toml_value(
            "package = \"foo\"\ngit = { url = \"u\", rev = \"abc\" }",
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("unknown `git` key `rev`"), "{err:#}");
    }

    #[test]
    fn merge_keeps_first_and_warns_on_version_conflict() {
        let mut deps = BTreeMap::new();
        let mut warnings = Vec::new();
        let a = parse_pub_ref(&toml_value("package = \"http\"\nversion = \"^1.2.0\"")).unwrap();
        let b = parse_pub_ref(&toml_value("package = \"http\"\nversion = \"^1.3.0\"")).unwrap();
        merge_dep(&mut deps, a, &mut warnings);
        merge_dep(&mut deps, b, &mut warnings);
        assert_eq!(deps["http"].version.as_deref(), Some("^1.2.0"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("conflicting versions"));
    }

    #[test]
    fn dep_entry_rendering_covers_every_source_shape() {
        let mut text = String::new();
        for spec in [
            "package = \"http\"\nversion = \"^1.2.0\"",
            "package = \"corp\"\nhosted = \"https://pub.corp.example\"\nversion = \"^2.0.0\"",
            "package = \"foo\"\nversion = \"^2.0.1\"\ngit = { url = \"https://g.example/foo\", ref = \"v1\", path = \"pkgs/foo\", tag_pattern = \"v{{version}}\" }",
            "package = \"local\"\npath = \"../local\"",
            "package = \"flutter\"\nsdk = \"flutter\"",
        ] {
            let r = parse_pub_ref(&toml_value(spec)).unwrap();
            write_dep_entry(&mut text, &r.package.clone(), &r);
        }
        let expected = "  http: \"^1.2.0\"
  corp:
    hosted: \"https://pub.corp.example\"
    version: \"^2.0.0\"
  foo:
    git:
      url: \"https://g.example/foo\"
      ref: \"v1\"
      path: \"pkgs/foo\"
      tag_pattern: \"v{{version}}\"
    version: \"^2.0.1\"
  local:
    path: \"../local\"
  flutter:
    sdk: flutter
";
        assert_eq!(text, expected);
    }

    /// The embedded PubReference schema and this module's hand-rolled
    /// parser describe the same grammar in two places. This test locks
    /// them together: for every case, the schema verdict and the parser
    /// verdict must agree (and match the expected verdict). A divergence
    /// means validate-passes-but-emit-fails (or the reverse) shipped.
    #[test]
    fn pub_reference_schema_and_parser_agree() {
        let schema = crate::embedded_schemas::embedded_schema_entries()
            .iter()
            .find(|(keys, _)| keys.iter().any(|k| k == "pub_reference.schema.yaml"))
            .expect("embedded PubReference schema")
            .1
            .pointer("/$defs/PubReference")
            .expect("$defs/PubReference present")
            .clone();
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");

        // (toml source, expected-accepted)
        let cases: &[(&str, bool)] = &[
            ("package = \"http\"\nversion = \"^1.2.0\"", true),
            (
                "package = \"corp\"\nhosted = \"https://pub.corp.example\"\nversion = \"^2.0.0\"",
                true,
            ),
            ("package = \"local\"\npath = \"../local\"", true),
            ("package = \"local\"\npath = \"../local\"\nversion = \"^1.0.0\"", true),
            (
                "package = \"foo\"\nversion = \"^2.0.1\"\ngit = { url = \"https://g.example/foo\", ref = \"v1\", path = \"pkgs/foo\", tag_pattern = \"v{{version}}\" }",
                true,
            ),
            ("package = \"foo\"\ngit = { url = \"https://g.example/foo\" }", true),
            ("package = \"flutter\"\nsdk = \"flutter\"", true),
            ("package = \"mocktail\"\nversion = \"^1.0.0\"\ntest_only = true", true),
            // Rejections.
            ("package = \"http\"", false),
            ("package = \"corp\"\nhosted = \"https://pub.corp.example\"", false),
            ("package = \"x\"\npth = \"../x\"", false),
            ("package = \"x\"\npath = \"../x\"\nsdk = \"flutter\"", false),
            ("package = \"foo\"\ngit = { ref = \"v1\" }", false),
            ("package = \"foo\"\ngit = { url = \"u\", rev = \"abc\" }", false),
            ("package = \"foo\"\ngit = { url = \"u\", ref = 1 }", false),
            ("package = \"http\"\nversion = 45", false),
            ("package = \"http\"\nversion = \"\"", false),
            ("package = \"http\"\nversion = \"^1.0.0\"\ntest_only = \"yes\"", false),
            ("package = \"\"\nversion = \"^1.0.0\"", false),
        ];

        for (src, expected_ok) in cases {
            let value = toml_value(src);
            let json = serde_json::to_value(&value).expect("toml → json");
            let schema_ok = validator.validate(&json).is_ok();
            let parser_ok = parse_pub_ref(&value).is_ok();
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

    #[test]
    fn app_version_and_flutter_config_parse_and_render() {
        let arch = toml_value(
            "version = \"1.2.3+45\"\n[flutter]\nuses_material_design = true\nassets = [\"assets/images/\"]",
        );
        let arch = arch.as_table().unwrap();
        assert_eq!(
            extract_app_version(arch).unwrap().as_deref(),
            Some("1.2.3+45")
        );
        let cfg = extract_flutter_config(arch).unwrap();
        assert_eq!(cfg.uses_material_design, Some(true));
        assert_eq!(cfg.assets, vec!["assets/images/".to_string()]);

        let mut text = String::new();
        write_flutter_section(&mut text, &cfg);
        assert_eq!(
            text,
            "flutter:\n  uses-material-design: true\n  assets:\n    - \"assets/images/\"\n"
        );

        // Empty config emits no header (a bare `flutter:` is YAML null).
        let mut empty = String::new();
        write_flutter_section(&mut empty, &FlutterConfig::default());
        assert!(empty.is_empty());
    }

    #[test]
    fn flutter_config_rejects_unknown_keys_and_wrong_types() {
        let arch = toml_value("[flutter]\nfonts = []");
        let err = extract_flutter_config(arch.as_table().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("unknown key `fonts`"), "{err:#}");

        let arch = toml_value("version = 45");
        let err = extract_app_version(arch.as_table().unwrap()).unwrap_err();
        assert!(format!("{err:#}").contains("must be a string"), "{err:#}");
    }

    #[test]
    fn pub_package_name_snakes_hyphens() {
        assert_eq!(pub_package_name("ble-scanner"), "ble_scanner");
        assert_eq!(pub_package_name("blescan"), "blescan");
    }

    #[test]
    fn pub_name_legality() {
        assert!(is_legal_pub_name("ble_scanner"));
        assert!(is_legal_pub_name("_private"));
        assert!(!is_legal_pub_name("BleScanner"), "uppercase is illegal");
        assert!(!is_legal_pub_name("9lives"), "leading digit is illegal");
        assert!(!is_legal_pub_name(""), "empty is illegal");
    }
}
