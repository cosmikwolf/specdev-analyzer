//! Shared fixture helpers for the checked-intent tests.
//!
//! A fixture is a `spec_version = "2.0"` project: bootstrapped from the
//! test plugin, the combined ledgers replaced by one file per entry, then
//! populated by the test. `check --format json` runs against it and the
//! test asserts on `(rule, entity, severity)` triples, never on message
//! text.
//!
//! A "silent" assertion first requires that `check` actually ran and
//! printed its JSON report. Without that guard, a missing subcommand would
//! satisfy every "this finding must not appear" assertion.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    inline::run_inline_block,
    layout::LayoutContext,
};

pub fn plugin_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-checked-intent-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// A fixture project on disk, and the plugin root it was built from.
pub struct Tree {
    pub root: PathBuf,
    pub plugin: PathBuf,
}

/// Bootstrap `profile` (Rust), inline `blocks`, and switch the tree to the
/// per-entry `2.0` layout with empty ledgers.
pub fn tree(tag: &str, profile: &str, blocks: &[&str]) -> Tree {
    tree_from(tag, &plugin_root(), profile, "rust", blocks)
}

/// As [`tree`], for another admitted language.
pub fn tree_lang(tag: &str, profile: &str, language: &str) -> Tree {
    tree_from(tag, &plugin_root(), profile, language, &[])
}

/// As [`tree`], from another plugin root (see [`plugin_with_block`]).
pub fn tree_from(tag: &str, plugin: &Path, profile: &str, language: &str, blocks: &[&str]) -> Tree {
    let root = tmp_dir(tag).join("proj");
    std::fs::create_dir_all(&root).unwrap();
    bootstrap::run_bootstrap(
        plugin,
        &BootstrapInputs {
            profile: profile.into(),
            language: language.into(),
            project_name: "checked_intent_fixture".into(),
            target_dir: root.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&root, None).unwrap();
    for b in blocks {
        run_inline_block(&layout, plugin, b).unwrap();
    }
    let t = Tree { root, plugin: plugin.to_path_buf() };
    to_v2(&t);
    t
}

/// A `1.x` project as bootstrap leaves it (combined ledgers), for `migrate`.
pub fn tree_v1(tag: &str, profile: &str, blocks: &[&str]) -> Tree {
    let root = tmp_dir(tag).join("proj");
    std::fs::create_dir_all(&root).unwrap();
    bootstrap::run_bootstrap(
        &plugin_root(),
        &BootstrapInputs {
            profile: profile.into(),
            language: "rust".into(),
            project_name: "checked_intent_fixture".into(),
            target_dir: root.clone(),
        },
    )
    .unwrap();
    let layout = LayoutContext::discover(&root, None).unwrap();
    for b in blocks {
        run_inline_block(&layout, &plugin_root(), b).unwrap();
    }
    Tree { root, plugin: plugin_root() }
}

fn to_v2(t: &Tree) {
    let root = &t.root;
    let spec = root.join("spec.toml");
    let text = std::fs::read_to_string(&spec).unwrap();
    assert!(text.starts_with("spec_version = \"1.0\""), "template changed: {text}");
    std::fs::write(&spec, text.replacen("spec_version = \"1.0\"", "spec_version = \"2.0\"", 1)).unwrap();
    for ledger in ["requirements.toml", "decisions.toml", "open_questions.toml"] {
        let _ = std::fs::remove_file(root.join("specs").join(ledger));
    }
    for dir in ["requirements", "decisions", "open_questions", "sources", "modules"] {
        std::fs::create_dir_all(root.join("specs").join(dir)).unwrap();
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dest);
        } else {
            std::fs::copy(e.path(), dest).unwrap();
        }
    }
}

/// A copy of the test plugin with one extra block `name`, from its schema
/// and reduction text. Returns the new plugin root.
pub fn plugin_with_block(tag: &str, name: &str, schema: &str, reduction: &str) -> PathBuf {
    let root = tmp_dir(&format!("{tag}-plugin")).join("_plugin");
    copy_dir(&plugin_root(), &root);
    let b = root.join("blocks").join(name);
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(b.join("schema.yaml"), schema).unwrap();
    std::fs::write(b.join("reduction.yaml"), reduction).unwrap();
    std::fs::write(b.join("README.md"), format!("# {name}\n\nTest-only block.\n")).unwrap();
    root
}

impl Tree {
    pub fn write(&self, rel: &str, content: &str) -> &Self {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
        self
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).unwrap()
    }

    pub fn analyzer(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
            .args(args)
            .arg("--no-color")
            .current_dir(&self.root)
            .env("SPECDEV_PLUGIN_ROOT", &self.plugin)
            .output()
            .expect("run specdev-analyzer")
    }

    /// `check --format json` plus `extra` arguments.
    pub fn check(&self, extra: &[&str]) -> Check {
        let mut args = vec!["check", "--format", "json"];
        args.extend_from_slice(extra);
        Check::from(self.analyzer(&args))
    }

    pub fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_AUTHOR_NAME", "fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
            .env("GIT_COMMITTER_NAME", "fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?} failed:\n{}", text(&out));
    }

    /// `git init` + commit everything, so `check --since` has a base.
    pub fn commit_all(&self, msg: &str) {
        if !self.root.join(".git").exists() {
            self.git(&["init", "-q", "-b", "main"]);
        }
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
    }
}

pub fn text(out: &Output) -> String {
    format!(
        "exit {:?}\n--- stdout\n{}\n--- stderr\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub rule: String,
    pub severity: String,
    pub entity: String,
}

/// One `check` run: its exit code, and its findings when the JSON parsed.
pub struct Check {
    pub exit: Option<i32>,
    pub report: Option<Vec<Finding>>,
    pub raw: String,
}

impl From<Output> for Check {
    fn from(out: Output) -> Self {
        let raw = text(&out);
        let report = serde_json::from_slice::<serde_json::Value>(&out.stdout)
            .ok()
            .and_then(|v| v.get("findings").and_then(|f| f.as_array()).cloned())
            .map(|arr| {
                arr.iter()
                    .map(|f| Finding {
                        rule: f["rule"].as_str().unwrap_or_default().to_string(),
                        severity: f["severity"].as_str().unwrap_or_default().to_string(),
                        entity: f["entity"].as_str().unwrap_or_default().to_string(),
                    })
                    .collect()
            });
        Check { exit: out.status.code(), report, raw }
    }
}

impl Check {
    /// Panics unless `check` ran and printed a JSON report (exit 0 or 1).
    pub fn ran(&self) -> &Vec<Finding> {
        match (&self.report, self.exit) {
            (Some(f), Some(0 | 1)) => f,
            _ => panic!("`check` did not run and report:\n{}", self.raw),
        }
    }

    fn matching(&self, rule: &str, entity: &str) -> Vec<&Finding> {
        self.ran().iter().filter(|f| f.rule == rule && f.entity == entity).collect()
    }

    /// `rule` fired for `entity` at `severity`.
    #[track_caller]
    pub fn fires(&self, rule: &str, entity: &str, severity: &str) -> &Self {
        let hits = self.matching(rule, entity);
        assert!(
            hits.iter().any(|f| f.severity == severity),
            "expected {rule} {severity} for `{entity}`; got {hits:?}\n{}",
            self.raw
        );
        self
    }

    /// `rule` did not fire for `entity` at any severity.
    #[track_caller]
    pub fn silent(&self, rule: &str, entity: &str) -> &Self {
        let hits = self.matching(rule, entity);
        assert!(hits.is_empty(), "expected no {rule} for `{entity}`; got {hits:?}\n{}", self.raw);
        self
    }

    /// No finding of any rule is about `entity`.
    #[track_caller]
    pub fn nothing_about(&self, entity: &str) -> &Self {
        let hits: Vec<&Finding> = self.ran().iter().filter(|f| f.entity == entity).collect();
        assert!(hits.is_empty(), "expected no findings about `{entity}`; got {hits:?}\n{}", self.raw);
        self
    }

    #[track_caller]
    pub fn exit_is(&self, code: i32) -> &Self {
        self.ran();
        assert_eq!(self.exit, Some(code), "{}", self.raw);
        self
    }
}

/// A requirement entry file body.
pub fn requirement(id: &str, extra: &str) -> String {
    format!("[[requirement]]\nid = \"{id}\"\ntext = \"The system shall do {id}.\"\nnature = \"module\"\n{extra}")
}

/// A firmware-layered module spec body.
pub fn fw_module(name: &str, extra: &str) -> String {
    format!("name = \"{name}\"\nlayer = \"application\"\ndescription = \"Fixture module {name}.\"\n{extra}")
}
