//! `report decisions` shows which decision text reaches which module's
//! prompt (plan R1, and R2's candidate half).
//!
//! It is a projection over the function the prompt is rendered from, so the
//! load-bearing assertion is agreement: for every module, the decision and
//! open-question ids in the report are exactly those the prompt carries.

use crate::ledger;

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::bootstrap::{self, BootstrapInputs};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/specs/full_plugin/_plugin")
}

fn project(tag: &str) -> PathBuf {
    let t = std::env::temp_dir()
        .join(format!(
            "specdev-report-decisions-{tag}-{}",
            std::process::id()
        ))
        .join("proj");
    let _ = std::fs::remove_dir_all(&t);
    std::fs::create_dir_all(&t).unwrap();
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "demo".into(),
            target_dir: t.clone(),
        },
    )
    .unwrap();
    // Ledgers go through the per-entry helper; module specs are written as is.
    let w = |rel: &str, body: &str| {
        if let Some(combined) = rel.strip_prefix("specs/").filter(|r| !r.contains('/')) {
            ledger::write(&t, combined, body);
            return;
        }
        let p = t.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    };
    w(
        "specs/decisions.toml",
        r#"
[[decision]]
id = "DEC-001"
title = "Guided"
status = "accepted"
text = "guided text"
codegen_include = ["text", "alternatives"]
alternatives = "alt text"

[[decision]]
id = "DEC-002"
title = "Through a requirement"
status = "accepted"
text = "requirement text"

[[decision]]
id = "DEC-003"
title = "Old"
status = "superseded"
text = "old text"

[[decision]]
id = "DEC-004"
title = "Nobody links here"
status = "accepted"
text = "orphan text"
"#,
    );
    w(
        "specs/requirements.toml",
        r#"
[[requirement]]
id = "REQ-1"
text = "the thing"
decided_by = ["DEC-002", "DEC-003"]
"#,
    );
    w(
        "specs/open_questions.toml",
        r#"
[[open_question]]
id = "OQ-1"
title = "assumed"
text = "we assume X"
status = "default_committed"
affects = ["REQ-1"]
"#,
    );
    w(
        "specs/modules/Alpha.toml",
        "name = \"Alpha\"\nguided_by = [\"DEC-001\", \"DEC-003\"]\nsatisfies = [\"REQ-1\"]\n",
    );
    w(
        "specs/modules/Beta.toml",
        "name = \"Beta\"\nguided_by = [\"DEC-001\"]\n",
    );
    w("specs/modules/Gamma.toml", "name = \"Gamma\"\n");
    t
}

fn run(t: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .args(args)
        .arg("--project")
        .arg(t)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn report(t: &Path, extra: &[&str]) -> serde_yaml::Value {
    let mut args = vec!["report", "decisions"];
    args.extend_from_slice(extra);
    let (code, out, err) = run(t, &args);
    assert_eq!(code, 0, "{err}");
    serde_yaml::from_str(&out).unwrap()
}

fn ids(v: &serde_yaml::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|x| x.as_sequence())
        .map(|s| {
            s.iter()
                .map(|e| e["id"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn module<'a>(r: &'a serde_yaml::Value, name: &str) -> &'a serde_yaml::Value {
    r["modules"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|m| m["module"] == name)
        .unwrap_or_else(|| panic!("no module {name}"))
}

#[test]
fn per_module_view_names_each_decision_and_why() {
    let t = project("modules");
    let r = report(&t, &[]);
    let alpha = module(&r, "Alpha");
    // DEC-003 is superseded: named by both guided_by and REQ-1, reaches nothing.
    assert_eq!(ids(alpha, "decisions"), ["DEC-001", "DEC-002"]);
    let d1 = &alpha["decisions"][0];
    assert_eq!(d1["why"][0]["via"], "guided_by");
    assert_eq!(
        d1["fields"],
        serde_yaml::from_str::<serde_yaml::Value>("[text, alternatives]").unwrap()
    );
    let d2 = &alpha["decisions"][1];
    assert_eq!(d2["why"][0]["via"], "requirement");
    assert_eq!(d2["why"][0]["id"], "REQ-1");
    assert_eq!(
        d2["fields"],
        serde_yaml::from_str::<serde_yaml::Value>("[text]").unwrap(),
        "codegen_include absent: text + consequences, and this one has no consequences"
    );
    assert_eq!(ids(alpha, "open_questions"), ["OQ-1"]);
    assert_eq!(alpha["open_questions"][0]["why"][0]["via"], "affects");

    // A module with no decisions is still listed, with nothing under it.
    assert!(module(&r, "Gamma").get("decisions").is_none());
}

#[test]
fn per_decision_view_shows_reach_including_none() {
    let t = project("decisions");
    let r = report(&t, &[]);
    let row = |id: &str| {
        r["decisions"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .unwrap()
            .clone()
    };
    let reach = |id: &str| -> Vec<String> {
        row(id)["reaches"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(reach("DEC-001"), ["Alpha", "Beta"]);
    assert_eq!(reach("DEC-002"), ["Alpha"]);
    // The R2 signal: accepted, and in no prompt.
    assert!(reach("DEC-004").is_empty());
    assert_eq!(row("DEC-004")["status"], "accepted");
    assert!(reach("DEC-003").is_empty());
    assert_eq!(row("DEC-003")["status"], "superseded");
}

#[test]
fn filters_narrow_the_views_and_reject_unknown_names() {
    let t = project("filters");
    let r = report(&t, &["--module", "Beta", "--decision", "DEC-002"]);
    assert_eq!(r["modules"].as_sequence().unwrap().len(), 1);
    assert_eq!(r["modules"][0]["module"], "Beta");
    // The decision view is computed over every module, not the filtered one.
    assert_eq!(r["decisions"].as_sequence().unwrap().len(), 1);
    assert_eq!(r["decisions"][0]["reaches"][0], "Alpha");

    let (code, _, err) = run(&t, &["report", "decisions", "--module", "Nope"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--module `Nope`"), "{err}");
    let (code, _, err) = run(&t, &["report", "decisions", "--decision", "DEC-999"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--decision `DEC-999`"), "{err}");
}
