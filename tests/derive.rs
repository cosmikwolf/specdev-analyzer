//! `specdev-analyzer derive` — files derived per module from the profile's
//! `derived_files` rules, recorded as `derived` lineage rows.
//!
//! The fixture is the `nextjs-app` profile from `tests/specs/full_plugin`
//! (seven rules, three shim templates) with a module set that hits every
//! rule: one `app`, one `layout`, two `page`s, one `route_handler`, every
//! `boundary_kind`, plus six modules no rule matches (`component`,
//! `service`, `server_action`, `model`). These tests run the real binary
//! so the CLI's line format and exit codes are what is exercised.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};
use specdev_analyzer::gen_lineage::{run_gen_plan, GenLineage, GenPlanCategory};
use specdev_analyzer::lang::adapter_for;
use specdev_analyzer::layout::LayoutContext;

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_workspace(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-derive-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    p
}

/// (module, role, extra TOML lines, body file under src/). Every role, every
/// boundary kind, all three environments.
const MODULES: &[(&str, &str, &str, &str)] = &[
    ("App", "app", "", "app.tsx"),
    (
        "MarketingLayout",
        "layout",
        "app_path = \"/(marketing)\"",
        "marketing_layout.tsx",
    ),
    (
        "EventsPage",
        "page",
        "app_path = \"/events\"",
        "events_page.tsx",
    ),
    (
        "EventDetailPage",
        "page",
        "app_path = \"/events/[id]\"\nclient_component = true",
        "event_detail_page.tsx",
    ),
    (
        "HealthRoute",
        "route_handler",
        "app_path = \"/api/health\"",
        "health_route.ts",
    ),
    (
        "EventsError",
        "boundary",
        "boundary_kind = \"error\"\napp_path = \"/events\"\nclient_component = true",
        "events_error.tsx",
    ),
    (
        "GlobalError",
        "boundary",
        "boundary_kind = \"global-error\"\napp_path = \"/\"\nclient_component = true",
        "global_error.tsx",
    ),
    (
        "EventsLoading",
        "boundary",
        "boundary_kind = \"loading\"\napp_path = \"/events\"",
        "events_loading.tsx",
    ),
    (
        "NotFound",
        "boundary",
        "boundary_kind = \"not-found\"\napp_path = \"/\"",
        "not_found.tsx",
    ),
    (
        "RootTemplate",
        "boundary",
        "boundary_kind = \"template\"\napp_path = \"/\"",
        "root_template.tsx",
    ),
    (
        "ModalDefault",
        "boundary",
        "boundary_kind = \"default\"\napp_path = \"/@modal\"",
        "modal_default.tsx",
    ),
    (
        "EventCard",
        "component",
        "client_component = true",
        "event_card.tsx",
    ),
    ("EventList", "component", "", "event_list.tsx"),
    ("CreateEvent", "server_action", "", "create_event.ts"),
    (
        "EventRepository",
        "service",
        "environment = \"server\"",
        "event_repository.ts",
    ),
    (
        "BrowserStore",
        "service",
        "environment = \"client\"",
        "browser_store.ts",
    ),
    (
        "DateFormat",
        "service",
        "environment = \"universal\"",
        "date_format.ts",
    ),
    ("Event", "model", "", "event.ts"),
];

/// Every derived target the module set produces, with the import specifier
/// each shim must carry.
const DERIVED: &[(&str, &str)] = &[
    ("app/layout.tsx", "../src/app"),
    ("app/(marketing)/layout.tsx", "../../src/marketing_layout"),
    ("app/events/page.tsx", "../../src/events_page"),
    ("app/events/[id]/page.tsx", "../../../src/event_detail_page"),
    ("app/api/health/route.ts", "../../../src/health_route"),
    ("app/events/error.tsx", "../../src/events_error"),
    ("app/global-error.tsx", "../src/global_error"),
    ("app/events/loading.tsx", "../../src/events_loading"),
    ("app/not-found.tsx", "../src/not_found"),
    ("app/template.tsx", "../src/root_template"),
    ("app/@modal/default.tsx", "../../src/modal_default"),
];

fn write_module(project: &Path, name: &str, role: &str, extra: &str) {
    fs::write(
        project.join(format!("specs/modules/{name}.toml")),
        format!("name = \"{name}\"\nrole = \"{role}\"\n{extra}\ndescription = \"{name}.\"\n"),
    )
    .unwrap();
}

/// Bootstrap a nextjs-app project from the fixture plugin, author the
/// module set, and write a body for every module.
fn fresh(tag: &str) -> PathBuf {
    let project = tmp_workspace(tag).join("proj");
    fs::create_dir_all(&project).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "nextjs-app".into(),
            language: "typescript".into(),
            project_name: "hello_next".into(),
            target_dir: project.clone(),
        },
    )
    .unwrap();
    fs::create_dir_all(project.join("specs/modules")).unwrap();
    fs::create_dir_all(project.join("src")).unwrap();
    fs::create_dir_all(project.join("app")).unwrap();
    for (name, role, extra, body) in MODULES {
        write_module(&project, name, role, extra);
        fs::write(
            project.join("src").join(body),
            format!("// body of {name}\n"),
        )
        .unwrap();
    }
    project
}

fn run(project: &Path, sub: &str, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .arg(sub)
        .arg("--project")
        .arg(project)
        .args(args)
        .env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .output()
        .expect("spawn specdev-analyzer");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn derive(project: &Path, args: &[&str]) -> (i32, String, String) {
    run(project, "derive", args)
}

fn lines_with(stdout: &str, outcome: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.starts_with(&format!("{outcome} ")))
        .map(str::to_string)
        .collect()
}

fn read(project: &Path, rel: &str) -> String {
    fs::read_to_string(project.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn lineage(project: &Path) -> GenLineage {
    GenLineage::load(&project.join("specs/specdev")).unwrap()
}

fn app_files(project: &Path) -> Vec<String> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        for e in fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, root, out);
            } else {
                out.push(
                    p.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let mut out = Vec::new();
    walk(&project.join("app"), project, &mut out);
    out.retain(|p| p != "app/.gitkeep");
    out.sort();
    out
}

// ---------------------------------------------------------------------------

#[test]
fn every_rule_renders_its_form_and_two_runs_are_byte_equal() {
    let project = fresh("forms");
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(lines_with(&out, "wrote").len(), DERIVED.len(), "{out}");
    assert!(lines_with(&out, "skipped").is_empty(), "{out}");
    assert_eq!(app_files(&project).len(), DERIVED.len());

    for (rel, spec) in DERIVED {
        let body = read(&project, rel);
        assert!(
            body.contains(&format!("\"{spec}\"")),
            "{rel} must import {spec}:\n{body}"
        );
        assert!(
            !body.contains("{source_relative}") && !body.contains("{project_name}"),
            "{rel} must carry no token:\n{body}"
        );
    }
    // The three forms.
    let error = read(&project, "app/events/error.tsx");
    assert!(error.starts_with("\"use client\";\n"), "{error}");
    assert_eq!(error.lines().count(), 3, "{error}");
    assert!(read(&project, "app/global-error.tsx").starts_with("\"use client\";\n"));
    let page = read(&project, "app/events/page.tsx");
    assert_eq!(page.lines().count(), 2, "{page}");
    assert!(page.contains("export { default } from"), "{page}");
    assert!(!page.contains("use client"), "{page}");
    let route = read(&project, "app/api/health/route.ts");
    assert_eq!(route, "export * from \"../../../src/health_route\";\n");

    // Recorded as derived rows under their modules, with an input hash.
    let l = lineage(&project);
    assert_eq!(l.files.len(), DERIVED.len());
    for (rel, _) in DERIVED {
        let row = &l.files[*rel];
        assert!(row.derived, "{rel} must be a derived row");
        assert!(
            row.module.is_some() && row.input_hash.is_some(),
            "{rel}: {row:?}"
        );
    }
    let lineage_text = read(&project, "specs/specdev/gen-lineage.toml");
    assert!(lineage_text.contains("derived = true"), "{lineage_text}");
    let before: Vec<(String, String)> = DERIVED
        .iter()
        .map(|(r, _)| ((*r).to_string(), read(&project, r)))
        .collect();

    // Second run: everything kept, nothing changes on disk or in lineage.
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(lines_with(&out, "kept").len(), DERIVED.len(), "{out}");
    assert!(lines_with(&out, "wrote").is_empty(), "{out}");
    assert_eq!(
        read(&project, "specs/specdev/gen-lineage.toml"),
        lineage_text,
        "lineage must not churn"
    );
    for (rel, bytes) in &before {
        assert_eq!(&read(&project, rel), bytes, "{rel}");
    }

    // The report is sorted by path.
    let paths: Vec<&str> = out.lines().map(|l| l.split_once(' ').unwrap().1).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted, "{out}");
}

#[test]
fn preview_without_write_touches_nothing() {
    let project = fresh("preview");
    let (code, out, err) = derive(&project, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(lines_with(&out, "wrote").len(), DERIVED.len(), "{out}");
    assert!(app_files(&project).is_empty());
    assert!(!project.join("specs/specdev/gen-lineage.toml").exists());
}

#[test]
fn unreachable_rule_set_is_refused_before_any_write() {
    let project = fresh("unreachable");
    let p = project.join("specs/specdev/profile.yaml");
    let body = fs::read_to_string(&p).unwrap();
    // Move the general boundary rule ahead of the two error-kind rules.
    let general = "  - when: { role: boundary }\n    path: \"app${.app_path}/${.boundary_kind}.tsx\"\n    template: templates/shim-default.tsx.template\n";
    assert!(body.contains(general), "{body}");
    let body = body.replace(general, "");
    let body = body.replace("derived_files:\n", &format!("derived_files:\n{general}"));
    fs::write(&p, body).unwrap();

    let (code, _out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("unreachable"), "{err}");
    assert!(err.contains("rule 1") && err.contains("rule 2"), "{err}");
    assert!(err.contains("role: boundary"), "{err}");
    assert!(
        app_files(&project).is_empty(),
        "nothing is written when the set is refused"
    );
}

#[test]
fn page_and_route_handler_in_one_segment_are_refused() {
    let project = fresh("exclusive");
    write_module(&project, "HealthPage", "page", "app_path = \"/api/health\"");
    fs::write(project.join("src/health_page.tsx"), "// body\n").unwrap();
    let (code, _out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("exclusive group `segment`"), "{err}");
    assert!(
        err.contains("`HealthPage`") && err.contains("`HealthRoute`"),
        "{err}"
    );
    assert!(
        app_files(&project).is_empty(),
        "nothing is written when the set is refused"
    );
}

#[test]
fn same_target_from_two_modules_is_refused_case_folded() {
    let project = fresh("collide");
    write_module(&project, "EventsPageTwo", "page", "app_path = \"/Events\"");
    fs::write(project.join("src/events_page_two.tsx"), "// body\n").unwrap();
    let (code, _out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("`EventsPage`") && err.contains("`EventsPageTwo`"),
        "{err}"
    );
    assert!(err.contains("case-insensitive"), "{err}");
    assert!(app_files(&project).is_empty());
}

#[test]
fn app_path_change_deletes_the_old_shim_and_its_row() {
    let project = fresh("move");
    derive(&project, &["--write"]);
    write_module(&project, "EventsPage", "page", "app_path = \"/happenings\"");
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("deleted app/events/page.tsx"), "{out}");
    assert!(out.contains("wrote app/happenings/page.tsx"), "{out}");
    assert!(!project.join("app/events/page.tsx").exists());
    assert!(project.join("app/happenings/page.tsx").is_file());
    let l = lineage(&project);
    assert!(!l.files.contains_key("app/events/page.tsx"));
    assert!(l.files["app/happenings/page.tsx"].derived);
    // The other files in that directory are untouched.
    assert!(project.join("app/events/error.tsx").is_file());
}

#[test]
fn deleted_module_removes_its_shim_and_a_missing_file_removes_its_row() {
    let project = fresh("stale");
    derive(&project, &["--write"]);
    fs::remove_file(project.join("specs/modules/HealthRoute.toml")).unwrap();
    // A second stale row whose file is already gone.
    fs::remove_file(project.join("specs/modules/NotFound.toml")).unwrap();
    fs::remove_file(project.join("app/not-found.tsx")).unwrap();
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("deleted app/api/health/route.ts"), "{out}");
    assert!(out.contains("deleted app/not-found.tsx"), "{out}");
    assert!(!project.join("app/api/health/route.ts").exists());
    let l = lineage(&project);
    assert!(!l.files.contains_key("app/api/health/route.ts"));
    assert!(!l.files.contains_key("app/not-found.tsx"));
}

#[test]
fn edited_shim_is_reported_kept_and_only_accept_edits_overwrites() {
    let project = fresh("edited");
    derive(&project, &["--write"]);
    let shim = project.join("app/events/page.tsx");
    let original = fs::read_to_string(&shim).unwrap();
    let edited = format!("{original}export const dynamic = \"force-dynamic\";\n");
    fs::write(&shim, &edited).unwrap();

    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "an edited line is a report, not a failure\n{err}");
    assert_eq!(
        lines_with(&out, "edited"),
        vec!["edited app/events/page.tsx".to_string()],
        "{out}"
    );
    assert_eq!(
        fs::read_to_string(&shim).unwrap(),
        edited,
        "never overwritten silently"
    );
    // The row still carries the old hash: gen-drift would see it, derive
    // reports it again next run.
    let (_, out, _) = derive(&project, &["--write"]);
    assert_eq!(lines_with(&out, "edited").len(), 1, "{out}");

    let (code, out, err) = derive(&project, &["--write", "--accept-edits"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("wrote app/events/page.tsx"), "{out}");
    assert_eq!(fs::read_to_string(&shim).unwrap(), original);
}

#[test]
fn a_hand_edit_equal_to_the_desired_bytes_is_kept_not_edited() {
    let project = fresh("equal-edit");
    derive(&project, &["--write"]);
    // Rename the body to the directory-module form: desired bytes change
    // (the specifier collapses to the same string — a no-op here), so
    // use a template edit to move D, then hand-write exactly D.
    let t = project.join("specs/specdev/templates/shim-star.ts.template");
    fs::write(&t, "// hand-equal\nexport * from \"{source_relative}\";\n").unwrap();
    let shim = project.join("app/api/health/route.ts");
    fs::write(
        &shim,
        "// hand-equal\nexport * from \"../../../src/health_route\";\n",
    )
    .unwrap();
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("kept app/api/health/route.ts"), "{out}");
    assert!(lines_with(&out, "edited").is_empty(), "{out}");
}

#[test]
fn stale_shim_with_a_hand_edit_is_kept_and_reported() {
    let project = fresh("stale-edited");
    derive(&project, &["--write"]);
    let shim = project.join("app/api/health/route.ts");
    fs::write(&shim, "// hand edit\n").unwrap();
    fs::remove_file(project.join("specs/modules/HealthRoute.toml")).unwrap();
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    let line = lines_with(&out, "edited");
    assert_eq!(line.len(), 1, "{out}");
    assert!(
        line[0].contains("stale derived file kept because of a hand edit"),
        "{out}"
    );
    assert!(shim.is_file());
    assert!(
        lineage(&project)
            .files
            .contains_key("app/api/health/route.ts"),
        "row kept"
    );

    let (code, out, err) = derive(&project, &["--write", "--accept-edits"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("deleted app/api/health/route.ts"), "{out}");
    assert!(!shim.exists());
    assert!(!lineage(&project)
        .files
        .contains_key("app/api/health/route.ts"));
}

#[test]
fn template_edit_rewrites_a_clean_shim() {
    // F == R and D != F: the file is still what derive wrote, but the
    // desired bytes moved. Comparing F to R alone would keep the old
    // shim forever.
    let project = fresh("template");
    derive(&project, &["--write"]);
    let t = project.join("specs/specdev/templates/shim-star.ts.template");
    fs::write(&t, "// v2\nexport * from \"{source_relative}\";\n").unwrap();
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("wrote app/api/health/route.ts"), "{out}");
    assert!(read(&project, "app/api/health/route.ts").starts_with("// v2\n"));
    // Everything else is kept.
    assert_eq!(lines_with(&out, "kept").len(), DERIVED.len() - 1, "{out}");
}

#[test]
fn unrecorded_file_at_target_is_a_conflict_until_accepted() {
    let project = fresh("conflict");
    fs::create_dir_all(project.join("app/events")).unwrap();
    let shim = project.join("app/events/page.tsx");
    fs::write(&shim, "// hand-written route file\n").unwrap();
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        lines_with(&out, "conflict"),
        vec!["conflict app/events/page.tsx: unrecorded file at target".to_string()],
        "{out}"
    );
    assert_eq!(
        fs::read_to_string(&shim).unwrap(),
        "// hand-written route file\n"
    );
    assert!(
        !lineage(&project).files.contains_key("app/events/page.tsx"),
        "a conflict records nothing"
    );
    // The other shims are written normally.
    assert_eq!(lines_with(&out, "wrote").len(), DERIVED.len() - 1, "{out}");

    let (code, out, err) = derive(&project, &["--write", "--accept-edits"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("wrote app/events/page.tsx"), "{out}");
    assert!(lineage(&project).files["app/events/page.tsx"].derived);
}

#[test]
fn unrecorded_file_equal_to_desired_is_recorded_as_kept() {
    let project = fresh("adopt");
    fs::create_dir_all(project.join("app/api/health")).unwrap();
    fs::write(
        project.join("app/api/health/route.ts"),
        "export * from \"../../../src/health_route\";\n",
    )
    .unwrap();
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("kept app/api/health/route.ts"), "{out}");
    assert!(lineage(&project).files["app/api/health/route.ts"].derived);
}

#[test]
fn body_less_module_is_skipped_and_reads_new_in_gen_plan() {
    let project = fresh("nobody");
    fs::remove_file(project.join("src/events_page.tsx")).unwrap();
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        lines_with(&out, "skipped"),
        vec!["skipped EventsPage: no body".to_string()],
        "{out}"
    );
    assert!(!project.join("app/events/page.tsx").exists());
    assert!(!out.contains("app/events/page.tsx"), "{out}");

    // The body arrives; the shim follows.
    fs::write(project.join("src/events_page.tsx"), "// body\n").unwrap();
    let (_, out, _) = derive(&project, &["--write"]);
    assert!(out.contains("wrote app/events/page.tsx"), "{out}");

    // The body goes again: the shim is stale and is deleted.
    fs::remove_file(project.join("src/events_page.tsx")).unwrap();
    let (_, out, _) = derive(&project, &["--write"]);
    assert!(out.contains("skipped EventsPage: no body"), "{out}");
    assert!(out.contains("deleted app/events/page.tsx"), "{out}");

    // A module whose only rows are derived, and whose body is gone, reads
    // `new`. The fixture's other modules have hand-written bodies at the
    // convention path and no body rows, so they read `unrecorded` (#216).
    let layout = LayoutContext::discover(&project, None).unwrap();
    let plan = run_gen_plan(&layout).unwrap();
    for m in &plan.modules {
        let expected = if m.module == "EventsPage" {
            GenPlanCategory::New
        } else {
            GenPlanCategory::Unrecorded
        };
        assert_eq!(m.category, expected, "{}: {:?}", m.module, m);
    }
}

#[test]
fn derived_rows_never_change_the_gen_plan_verdict() {
    let project = fresh("plan");
    // Record the body as the codegen skill would, then derive.
    let body = project.join("src/health_route.ts");
    let (code, _, err) = run(
        &project,
        "gen-record",
        &[
            "--accept-edits",
            "--module",
            "HealthRoute",
            body.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");

    let layout = LayoutContext::discover(&project, None).unwrap();
    let verdict = |name: &str| {
        run_gen_plan(&layout)
            .unwrap()
            .modules
            .into_iter()
            .find(|m| m.module == name)
            .unwrap()
    };
    // The body row and the derived row carry one hash (the quiet hash
    // equals the recording hash), so the module is unchanged.
    let l = lineage(&project);
    assert_eq!(
        l.files["src/health_route.ts"].input_hash, l.files["app/api/health/route.ts"].input_hash,
        "derive and gen-record must compute one input hash"
    );
    assert_eq!(verdict("HealthRoute").category, GenPlanCategory::Unchanged);

    // A missing derived file does not make the module missing.
    fs::remove_file(project.join("app/api/health/route.ts")).unwrap();
    let v = verdict("HealthRoute");
    assert_eq!(v.category, GenPlanCategory::Unchanged, "{v:?}");
    assert!(v.files.is_empty(), "{v:?}");
    derive(&project, &["--write"]);

    // An edited derived file does not make the module edited.
    fs::write(project.join("app/api/health/route.ts"), "// edit\n").unwrap();
    let v = verdict("HealthRoute");
    assert_eq!(v.category, GenPlanCategory::Unchanged, "{v:?}");

    // An edited body still does.
    fs::write(&body, "// edited body\n").unwrap();
    assert_eq!(verdict("HealthRoute").category, GenPlanCategory::Edited);
}

#[test]
fn gen_record_refuses_a_derived_path() {
    let project = fresh("gen-record");
    derive(&project, &["--write"]);
    let shim = project.join("app/events/page.tsx");
    let (code, _, err) = run(
        &project,
        "gen-record",
        &[
            "--accept-edits",
            "--module",
            "EventsPage",
            shim.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("derived file"), "{err}");
    assert!(err.contains("derive"), "{err}");
    assert!(
        lineage(&project).files["app/events/page.tsx"].derived,
        "the flag survives"
    );
}

#[test]
fn gen_record_refuses_a_path_owned_by_a_live_module_and_reassigns_a_stale_owner() {
    let project = fresh("ownership");
    let shared = project.join("src/event.ts");
    let (code, _, err) = run(
        &project,
        "gen-record",
        &[
            "--accept-edits",
            "--module",
            "Event",
            shared.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = run(
        &project,
        "gen-record",
        &[
            "--accept-edits",
            "--module",
            "EventList",
            shared.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("already recorded for module `Event`"), "{err}");
    assert_eq!(
        lineage(&project).files["src/event.ts"].module.as_deref(),
        Some("Event")
    );

    // The owner is deleted: the path changes hands with a note.
    fs::remove_file(project.join("specs/modules/Event.toml")).unwrap();
    let (code, _, err) = run(
        &project,
        "gen-record",
        &[
            "--accept-edits",
            "--module",
            "EventList",
            shared.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("reassigned"), "{err}");
    assert_eq!(
        lineage(&project).files["src/event.ts"].module.as_deref(),
        Some("EventList")
    );
}

#[test]
fn derive_refuses_a_target_recorded_for_another_live_module() {
    let project = fresh("derive-ownership");
    // The skill (wrongly) recorded a body file at a derived target under
    // another module.
    fs::create_dir_all(project.join("app/events")).unwrap();
    let target = project.join("app/events/page.tsx");
    fs::write(&target, "// something\n").unwrap();
    let (code, _, err) = run(
        &project,
        "gen-record",
        &[
            "--accept-edits",
            "--module",
            "EventList",
            target.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("already recorded for module `EventList`"),
        "{err}"
    );
}

#[test]
fn stale_inlined_profile_is_refused_with_the_block_to_paste() {
    let project = fresh("stale-profile");
    let p = project.join("specs/specdev/profile.yaml");
    let body = fs::read_to_string(&p).unwrap();
    let cut = body.find("derived_files:").unwrap();
    let (head, tail) = body.split_at(cut);
    let rest = tail.find("\nbootstrap:").unwrap();
    fs::write(&p, format!("{head}{}", &tail[rest + 1..])).unwrap();
    assert!(!fs::read_to_string(&p).unwrap().contains("derived_files"));

    let plugin = full_plugin();
    let (code, out, err) = derive(
        &project,
        &["--write", "--plugin-root", plugin.to_str().unwrap()],
    );
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("stale"), "{err}");
    assert!(err.contains("derived_files:"), "{err}");
    assert!(err.contains("role: route_handler"), "{err}");
    assert!(err.contains("exclusive: segment"), "{err}");
    assert!(app_files(&project).is_empty());

    // Without a plugin root the same project gives the plugin-root error,
    // not a silent no-op.
    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("PLUGIN-001"), "{err}");
}

#[test]
fn a_profile_with_no_derived_files_is_a_silent_no_op() {
    let project = tmp_workspace("rust").join("proj");
    fs::create_dir_all(&project).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "mylib".into(),
            target_dir: project.clone(),
        },
    )
    .unwrap();
    let plugin = full_plugin();
    let (code, out, err) = derive(
        &project,
        &["--write", "--plugin-root", plugin.to_str().unwrap()],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.is_empty(), "{out}");
    assert!(!project.join("specs/specdev/gen-lineage.toml").exists());
}

#[test]
fn missing_template_is_an_error_before_any_write() {
    let project = fresh("template-missing");
    fs::remove_file(project.join("specs/specdev/templates/shim-star.ts.template")).unwrap();
    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("shim-star.ts.template"), "{err}");
    assert!(
        app_files(&project).is_empty(),
        "nothing written when a template is missing"
    );
}

#[test]
fn a_directory_at_a_target_is_an_error_naming_the_path() {
    let project = fresh("dir");
    fs::create_dir_all(project.join("app/events/page.tsx")).unwrap();
    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("app/events/page.tsx") && err.contains("directory"),
        "{err}"
    );
}

#[test]
fn a_target_outside_the_project_root_is_refused() {
    let project = fresh("escape");
    let p = project.join("specs/specdev/profile.yaml");
    let body = fs::read_to_string(&p).unwrap();
    let body = body.replace(
        "    path: app/layout.tsx\n",
        "    path: ../outside/layout.tsx\n",
    );
    fs::write(&p, body).unwrap();
    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("inside the project root"), "{err}");
    assert!(!project.parent().unwrap().join("outside").exists());
    assert!(app_files(&project).is_empty());
}

#[test]
fn every_token_occurrence_is_replaced_and_other_braces_are_not_tokens() {
    let project = fresh("token");
    let t = project.join("specs/specdev/templates/shim-star.ts.template");
    fs::write(
        &t,
        "export * from \"{source_relative}\"; // {project_name} {source_relative} {other} {{x}}\n",
    )
    .unwrap();
    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        read(&project, "app/api/health/route.ts"),
        "export * from \"../../../src/health_route\"; // hello_next ../../../src/health_route {other} {{x}}\n"
    );
}

#[test]
fn source_file_override_and_directory_module_form_drive_the_specifier() {
    let project = fresh("specifier");
    // Directory-module body: `<snake>/index.tsx` imports as `<snake>`.
    fs::remove_file(project.join("src/events_page.tsx")).unwrap();
    fs::create_dir_all(project.join("src/events_page")).unwrap();
    fs::write(project.join("src/events_page/index.tsx"), "// body\n").unwrap();
    // Explicit override elsewhere in the tree.
    write_module(
        &project,
        "HealthRoute",
        "route_handler",
        "app_path = \"/api/health\"\nsource_file = \"src/api/health.ts\"",
    );
    fs::create_dir_all(project.join("src/api")).unwrap();
    fs::write(project.join("src/api/health.ts"), "// body\n").unwrap();

    let (code, _, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(read(&project, "app/events/page.tsx").contains("\"../../src/events_page\""));
    assert_eq!(
        read(&project, "app/api/health/route.ts"),
        "export * from \"../../../src/api/health\";\n"
    );
}

#[test]
fn import_specifier_is_a_language_fact() {
    let ts = adapter_for("typescript").unwrap();
    assert_eq!(
        ts.import_specifier(Path::new("app/events"), Path::new("src/events_page.tsx")),
        "../../src/events_page"
    );
    assert_eq!(
        ts.import_specifier(Path::new("app"), Path::new("src/app.ts")),
        "../src/app"
    );
    assert_eq!(
        ts.import_specifier(Path::new("app/x"), Path::new("src/foo/index.tsx")),
        "../../src/foo"
    );
    assert_eq!(
        ts.import_specifier(Path::new("src"), Path::new("src/foo.ts")),
        "./foo"
    );
    assert_eq!(
        ts.import_specifier(Path::new("src/foo"), Path::new("src/foo/index.ts")),
        "."
    );
    // The default keeps the extension: a Dart import names the file.
    let dart = adapter_for("dart").unwrap();
    assert_eq!(
        dart.import_specifier(Path::new("lib"), Path::new("lib/src/foo.dart")),
        "./src/foo.dart"
    );
    assert_eq!(
        dart.import_specifier(Path::new("lib/a/b"), Path::new("lib/c.dart")),
        "../../c.dart"
    );
}

#[test]
fn nextjs_app_profile_declares_seven_rules_in_specific_to_general_order() {
    let project = fresh("decl");
    let profile =
        specdev_analyzer::manifest::Profile::load(&project.join("specs/specdev/profile.yaml"))
            .unwrap();
    assert_eq!(profile.derived_files.len(), 7);
    for r in &profile.derived_files {
        assert!(
            project.join("specs/specdev").join(&r.template).is_file(),
            "{}",
            r.template
        );
    }
    assert_eq!(
        profile
            .derived_files
            .iter()
            .filter(|r| r.exclusive.as_deref() == Some("segment"))
            .count(),
        2
    );
}

#[test]
fn case_only_app_path_rename_never_deletes_the_live_shim() {
    // On a case-insensitive file system the old key and the new target are
    // one file; on a case-sensitive one they are two. Both must end with the
    // new target present and the old row gone, and a further run all `kept`.
    let project = fresh("case-rename");
    derive(&project, &["--write"]);
    write_module(&project, "EventsPage", "page", "app_path = \"/Events\"");
    write_module(
        &project,
        "EventsError",
        "boundary",
        "boundary_kind = \"error\"\napp_path = \"/Events\"\nclient_component = true",
    );
    write_module(
        &project,
        "EventsLoading",
        "boundary",
        "boundary_kind = \"loading\"\napp_path = \"/Events\"",
    );
    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("deleted app/events/page.tsx"), "{out}");
    assert!(project.join("app/Events/page.tsx").is_file(), "{out}");
    assert!(read(&project, "app/Events/page.tsx").contains("\"../../src/events_page\""));
    let l = lineage(&project);
    assert!(!l.files.contains_key("app/events/page.tsx"));
    assert!(l.files["app/Events/page.tsx"].derived);

    let (code, out, err) = derive(&project, &["--write"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(lines_with(&out, "kept").len(), DERIVED.len(), "{out}");
    assert!(
        lines_with(&out, "wrote").is_empty() && lines_with(&out, "deleted").is_empty(),
        "{out}"
    );
}
