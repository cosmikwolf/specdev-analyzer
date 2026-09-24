//! `match:` relations (built from entity attributes, not spec fields),
//! the `contains` / `contained_by` context buckets, and the
//! `entity_count` graph check — exercised through the nextjs-app
//! profile's `renders` relation and `single_app_module` rule.

use std::path::{Path, PathBuf};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    context,
    layout::LayoutContext,
    reduction, validate,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn fresh_nextjs(tag: &str) -> PathBuf {
    let target = std::env::temp_dir().join(format!(
        "specdev-render-containment-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_dir_all(&target);
    std::fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "nextjs-app".into(),
            language: "typescript".into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

fn module(root: &Path, name: &str, role: &str, extra: &str) {
    let dir = root.join("specs/modules");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.toml")),
        format!("name = \"{name}\"\nrole = \"{role}\"\n{extra}"),
    )
    .unwrap();
}

fn single_app_findings(root: &Path) -> Vec<String> {
    let layout = LayoutContext::discover(root, None).unwrap();
    let report = validate::run(&layout).unwrap();
    report
        .findings()
        .iter()
        .filter(|f| f.rule == "single_app_module")
        .map(|f| f.message.clone())
        .collect()
}

#[test]
fn app_contains_pages_layouts_and_boundaries_but_not_services() {
    let root = fresh_nextjs("cone");
    module(&root, "App", "app", "");
    module(
        &root,
        "Home",
        "page",
        "app_path = \"/\"\ndepends_on = [\"Repo\"]\n",
    );
    module(&root, "Shell", "layout", "app_path = \"/a\"\n");
    module(
        &root,
        "Missing",
        "boundary",
        "boundary_kind = \"not-found\"\n",
    );
    module(&root, "Repo", "service", "");

    let layout = LayoutContext::discover(&root, None).unwrap();
    let app = context::run_context(&layout, "App").unwrap();
    assert_eq!(
        app.related_modules.contains,
        vec!["Home", "Missing", "Shell"]
    );
    assert!(app.related_modules.upstream.is_empty());
    assert!(app.related_modules.downstream.is_empty());
    // The contained modules count as related: their specs are listed and
    // they are not in the excluded count (5 modules - App - 3 contained).
    assert!(app
        .related_files
        .contains(&"specs/modules/Home.toml".to_string()));
    assert_eq!(app.excluded_module_count, 1);

    let home = context::run_context(&layout, "Home").unwrap();
    assert_eq!(home.related_modules.contained_by, vec!["App"]);
    assert_eq!(home.related_modules.upstream, vec!["Repo"]);

    let repo = context::run_context(&layout, "Repo").unwrap();
    assert!(repo.related_modules.contained_by.is_empty());
}

#[test]
fn renders_edges_carry_contains_and_stay_out_of_role_dependency() {
    let root = fresh_nextjs("layer");
    module(&root, "App", "app", "");
    module(&root, "Home", "page", "app_path = \"/\"\n");

    let (graph, _) = reduction::run_project(&root, None).unwrap();
    let renders: Vec<_> = graph
        .relations
        .iter()
        .filter(|r| r.kind == "renders")
        .collect();
    assert_eq!(renders.len(), 1);
    assert_eq!(renders[0].extends, "contains");
    assert_eq!(
        (renders[0].from.as_str(), renders[0].to.as_str()),
        ("App", "Home")
    );
    assert_eq!(renders[0].provenance.pointer, "/relations/renders");

    let layout = LayoutContext::discover(&root, None).unwrap();
    let report = validate::run(&layout).unwrap();
    assert!(
        !report
            .findings()
            .iter()
            .any(|f| f.rule == "role_dependency" || f.rule == "modules_acyclic"),
        "a containment edge must not reach the dependency checks"
    );
}

#[test]
fn single_app_module_counts_app_roles() {
    let root = fresh_nextjs("count");
    // No modules yet: nothing to count, no finding.
    assert!(single_app_findings(&root).is_empty());

    module(&root, "Home", "page", "app_path = \"/\"\n");
    let zero = single_app_findings(&root);
    assert_eq!(zero.len(), 1);
    assert!(zero[0].contains("0 `Module` entities"), "{zero:?}");

    module(&root, "App", "app", "");
    assert!(single_app_findings(&root).is_empty());

    module(&root, "App2", "app", "");
    let two = single_app_findings(&root);
    assert_eq!(two.len(), 1);
    assert!(two[0].contains("(App, App2)"), "{two:?}");
}

#[test]
fn malformed_match_is_an_error_not_an_empty_relation() {
    let root = fresh_nextjs("typo");
    module(&root, "App", "app", "");
    let red = root.join("specs/specdev/reductions/project.yaml");
    let text = std::fs::read_to_string(&red).unwrap();
    let bad = text.replace(
        "from: { attr: role, in: [app] }",
        "from: { atr: role, in: [app] }",
    );
    assert_ne!(
        text, bad,
        "fixture reduction no longer has the renders match"
    );
    std::fs::write(&red, bad).unwrap();

    let layout = LayoutContext::discover(&root, None).unwrap();
    let report = validate::run(&layout).unwrap();
    let hit = report
        .findings()
        .iter()
        .find(|f| f.rule == "METASCHEMA-006")
        .expect("METASCHEMA-006 for an unknown key in match.from");
    assert!(
        hit.message.contains("unknown key `atr` in `match.from`"),
        "{}",
        hit.message
    );
}
