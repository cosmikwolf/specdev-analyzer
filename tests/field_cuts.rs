//! The 2.0 field cuts: fields whose only reader was a manifest emitter, or
//! that copy the code, leave the format.
//!
//! What each test holds fixed:
//!
//! 1. No plugin schema, template, reduction or `domain.toml` template (live
//!    tree and fixture mirror) names a removed field, kind or check.
//! 2. A 1.x tree carrying each removed `spec.toml` key still loads, and
//!    `migrate` removes it.
//! 3. `migrate` on the frozen 1.x trees removes each field, edits the
//!    inlined reductions and `domain.toml`, deletes an `architecture.toml`
//!    with no key left, and the result validates clean.
//! 4. `reduce` before and after differs only by what the format removes or
//!    moves on purpose.
//! 5. The removal report prints value, comments and the right mark.
//! 6. `--dry-run` writes nothing.
//! 7. `context --module` prints `context_files`; `report decisions` still
//!    reads `codegen_include`.

use crate::ci;

use std::path::{Path, PathBuf};

use ci::{text, tree_v1, Tree};

/// Removed property paths, per schema file name. `[]` steps into an array's
/// items; a whole table is named by its key.
const REMOVED: &[(&str, &[&str])] = &[
    (
        "architecture",
        &[
            "external_deps",
            "features",
            "profiles",
            "test_targets",
            "workspace_dependencies",
            "dependency_overrides",
            "workspace_package",
            "msrv",
            "version",
            "environment",
            "scripts",
            "flutter.assets",
            "flutter.uses_material_design",
            "plugin.version",
            "platform.mcu_core",
            "platform.mcu_runtime",
            "platform.mcu_part",
            "runtime.allocator",
            "runtime.critical_section_impl",
            "runtime.realtime_audit",
            "runtime.panic_handler",
            "logger",
            "plugin",
            "audio",
        ],
    ),
    (
        "module",
        &["external_deps", "feature_gates", "actions[].params[].type", "actions[].returns", "errors[].variant"],
    ),
    ("item", &["test", "test_kind", "verifies"]),
    (
        "spec",
        &[
            "project.version",
            "project.edition",
            "project.license",
            "project.repository",
            "project.description",
            "workspace.resolver",
            "workspace.default_members",
            "workspace.exclude",
        ],
    ),
];

const REMOVED_KINDS: &[&str] =
    &["Feature", "feature_depends_on", "module_feature_gate", "WorkspaceDependency", "workspace_default_member", "Workspace", "test_verifies"];
const REMOVED_RULES: &[&str] = &["feature_unique_names", "workspace_dependencies_unique", "test_requirements_have_verifiers"];

fn roots() -> Vec<PathBuf> {
    let m = Path::new(env!("CARGO_MANIFEST_DIR"));
    vec![m.join("../specdev"), m.join("tests/specs/full_plugin/_plugin")]
}

/// Every property path in a schema, following local `$ref`s.
fn paths(schema: &serde_yaml::Value) -> Vec<String> {
    fn walk(node: &serde_yaml::Value, defs: &serde_yaml::Value, path: &str, seen: &mut Vec<String>, out: &mut Vec<String>) {
        let Some(map) = node.as_mapping() else { return };
        if let Some(r) = map.get("$ref").and_then(|r| r.as_str()) {
            if let Some(name) = r.strip_prefix("#/$defs/") {
                if !seen.contains(&name.to_string()) {
                    seen.push(name.to_string());
                    if let Some(d) = defs.get(name) {
                        walk(d, defs, path, seen, out);
                    }
                    seen.pop();
                }
            }
        }
        if let Some(props) = map.get("properties").and_then(|p| p.as_mapping()) {
            for (k, v) in props {
                let k = k.as_str().unwrap_or("");
                let p = if path.is_empty() { k.to_string() } else { format!("{path}.{k}") };
                out.push(p.clone());
                walk(v, defs, &p, seen, out);
            }
        }
        if let Some(items) = map.get("items") {
            walk(items, defs, &format!("{path}[]"), seen, out);
        }
        for key in ["allOf", "anyOf", "oneOf"] {
            if let Some(seq) = map.get(key).and_then(|s| s.as_sequence()) {
                for s in seq {
                    walk(s, defs, path, seen, out);
                }
            }
        }
        for key in ["then", "else"] {
            if let Some(s) = map.get(key) {
                walk(s, defs, path, seen, out);
            }
        }
    }
    let mut out = Vec::new();
    let defs = schema.get("$defs").cloned().unwrap_or(serde_yaml::Value::Null);
    walk(schema, &defs, "", &mut Vec::new(), &mut out);
    out
}

fn schema_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for p in glob(&root.join("profiles"), "schemas") {
        let stem = p.file_name().unwrap().to_string_lossy().replace(".schema.yaml", "");
        out.push((stem, p));
    }
    if let Ok(rd) = std::fs::read_dir(root.join("languages")) {
        for e in rd.flatten() {
            let p = e.path().join("items.schema.yaml");
            if p.is_file() {
                out.push(("item".into(), p));
            }
        }
    }
    out
}

fn glob(profiles: &Path, sub: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(profiles).unwrap().flatten() {
        if let Ok(rd) = std::fs::read_dir(e.path().join(sub)) {
            for f in rd.flatten() {
                if f.path().extension().is_some_and(|x| x == "yaml") {
                    out.push(f.path());
                }
            }
        }
    }
    out
}

#[test]
fn no_plugin_file_names_a_removed_field_kind_or_check() {
    let embedded = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/embedded/spec.schema.yaml");
    let mut files = vec![("spec".to_string(), embedded)];
    for root in roots() {
        files.extend(schema_files(&root));
    }
    let mut found = Vec::new();
    for (kind, path) in &files {
        let v: serde_yaml::Value = serde_yaml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let ps = paths(&v);
        for (k, removed) in REMOVED {
            if k != kind {
                continue;
            }
            for r in *removed {
                if ps.iter().any(|p| p == r) {
                    found.push(format!("{}: {r}", path.display()));
                }
            }
        }
    }
    for root in roots() {
        for e in std::fs::read_dir(root.join("profiles")).unwrap().flatten() {
            let red = std::fs::read_to_string(e.path().join("reductions/project.yaml")).unwrap_or_default();
            for k in REMOVED_KINDS {
                if red.lines().any(|l| l.trim() == format!("- kind: {k}")) {
                    found.push(format!("{}: reduction kind {k}", e.path().display()));
                }
            }
            let dom = std::fs::read_to_string(e.path().join("domain.toml.template")).unwrap_or_default();
            for r in REMOVED_RULES {
                if dom.contains(&format!("\"{r}\"")) || dom.lines().any(|l| l.trim_start().starts_with(&format!("{r} "))) {
                    found.push(format!("{}: domain rule {r}", e.path().display()));
                }
            }
            if let Ok(arch) = std::fs::read_to_string(e.path().join("architecture.toml.template")) {
                let doc: toml::Table = arch.replace("{{project_name}}", "x").parse().unwrap();
                for r in REMOVED.iter().find(|(k, _)| *k == "architecture").unwrap().1 {
                    let mut cur = Some(toml::Value::Table(doc.clone()));
                    for part in r.split('.') {
                        cur = cur.and_then(|c| c.get(part).cloned());
                    }
                    if cur.is_some() {
                        found.push(format!("{}: template key {r}", e.path().display()));
                    }
                }
            }
        }
    }
    assert!(found.is_empty(), "removed fields still named:\n  {}", found.join("\n  "));
}

fn migrate(t: &Tree, extra: &[&str]) -> std::process::Output {
    let plugin = t.plugin.to_str().unwrap().to_string();
    let mut args = vec!["migrate", "--plugin-root", plugin.as_str()];
    args.extend_from_slice(extra);
    t.analyzer(&args)
}

#[test]
fn a_1x_tree_with_removed_spec_keys_loads_and_migrate_removes_them() {
    let t = tree_v1("cuts-spec-keys", "library-api");
    let spec = t.read("spec.toml").replacen(
        "language = \"rust\"\n",
        "language = \"rust\"\nversion = \"0.1.0\"\nedition = \"2021\"\nlicense = \"MIT\"\nrepository = \"https://x\"\ndescription = \"A thing.\"\n",
        1,
    );
    t.write("spec.toml", &spec);
    let v = t.analyzer(&["validate", "-q"]);
    assert!(text(&v).lines().any(|l| l.starts_with("0 error(s)")), "{}", text(&v));
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let after: toml::Table = t.read("spec.toml").parse().unwrap();
    for k in ["version", "edition", "license", "repository", "description"] {
        assert!(after["project"].get(k).is_none(), "{k} left:\n{}", t.read("spec.toml"));
        assert!(text(&out).contains(&format!("[project].{k}")), "{k} not reported:\n{}", text(&out));
    }
    let v = t.analyzer(&["validate", "-q"]);
    assert!(text(&v).lines().any(|l| l.starts_with("0 error(s)")), "{}", text(&v));
}

/// A frozen 1.x tree with every removable field filled in.
fn v1_full(tag: &str, profile: &str) -> Tree {
    let t = tree_v1(tag, profile);
    t.write(
        "specs/modules/Core.toml",
        "name = \"Core\"\nlayer = \"application\"\ndescription = \"Core.\"\nfeature_gates = []\n\n# why nom: the parser\n[[external_deps]]\ncrate = \"nom\"\nversion = \"7\"\nsource = \"crates-io\"\n",
    );
    t
}

#[test]
fn migrate_removes_each_field_and_the_tree_validates() {
    let t = v1_full("cuts-fw", "firmware-layered");
    let arch = t.read("specs/architecture.toml");
    let out = migrate(&t, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let core: toml::Table = t.read("specs/modules/Core.toml").parse().unwrap();
    assert!(core.get("external_deps").is_none() && core.get("feature_gates").is_none(), "{core:?}");
    if t.root.join("specs/architecture.toml").exists() {
        let a: toml::Table = t.read("specs/architecture.toml").parse().unwrap();
        for k in ["runtime", "logger", "external_deps", "features", "test_targets", "profiles"] {
            assert!(a.get(k).is_none(), "{k} left in architecture.toml:\n{}", t.read("specs/architecture.toml"));
        }
        if let Some(p) = a.get("platform") {
            assert!(p.get("mcu_core").is_none() && p.get("mcu_part").is_none(), "{p:?}");
        }
    }
    let red = t.read("specs/specdev/reductions/project.yaml");
    for k in REMOVED_KINDS {
        assert!(!red.lines().any(|l| l.trim() == format!("- kind: {k}")), "{k} left in the reduction");
    }
    let dom = t.read("specs/specdev/domain.toml");
    for r in REMOVED_RULES {
        assert!(!dom.contains(r), "{r} left in domain.toml");
    }
    let v = t.analyzer(&["validate", "-q"]);
    assert!(text(&v).lines().any(|l| l.starts_with("0 error(s)")), "{}", text(&v));
    assert!(!arch.is_empty());
}

#[test]
fn an_architecture_file_with_no_key_left_is_deleted_only_when_the_template_has_none() {
    // library-api: the 2.0 template keeps no key, so a tree's file with only
    // removed keys goes.
    let t = tree_v1("cuts-arch-gone", "library-api");
    t.write("specs/architecture.toml", "msrv = \"1.75\"\n\n[[features]]\nname = \"std\"\ndefault = true\n");
    assert!(migrate(&t, &[]).status.success());
    assert!(!t.root.join("specs/architecture.toml").exists());
    let l: toml::Table = t.read("specs/specdev/lineage.toml").parse().unwrap();
    assert!(l.get("profile_files").and_then(|p| p.get("specs/architecture.toml")).is_none(), "record dropped");
    // firmware-layered: the template keeps `platform.target_triple`, so the file stays.
    let f = tree_v1("cuts-arch-stays", "firmware-layered");
    assert!(migrate(&f, &[]).status.success());
    assert!(f.root.join("specs/architecture.toml").exists());
}

#[test]
fn reduce_differs_only_by_what_the_format_removes() {
    let t = v1_full("cuts-reduce", "library-api");
    let before = graph(&t);
    assert!(migrate(&t, &[]).status.success());
    let after = graph(&t);
    let removed: Vec<&str> = REMOVED_KINDS.to_vec();
    let keep = |g: &serde_json::Value| -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        for e in g["entities"].as_array().into_iter().flatten() {
            let kind = e["kind"].as_str().unwrap_or("");
            if removed.iter().any(|r| kind.ends_with(r)) || kind.ends_with("Architecture") {
                continue;
            }
            let mut e = e.clone();
            e.as_object_mut().unwrap().remove("provenance");
            if let Some(a) = e.get_mut("attrs").and_then(|a| a.as_object_mut()) {
                for k in ["verified_by", "test_pending", "planned"] {
                    a.remove(k);
                }
            }
            v.push(e.to_string());
        }
        for r in g["relations"].as_array().into_iter().flatten() {
            let kind = r["kind"].as_str().unwrap_or("");
            if removed.iter().any(|x| kind.ends_with(x)) {
                continue;
            }
            let mut r = r.clone();
            r.as_object_mut().unwrap().remove("provenance");
            v.push(r.to_string());
        }
        v.sort();
        v
    };
    assert_eq!(keep(&before), keep(&after));
}

fn graph(t: &Tree) -> serde_json::Value {
    let out = t.analyzer(&["reduce"]);
    assert!(out.status.success(), "{}", text(&out));
    serde_yaml::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_report_marks_every_removed_value() {
    // A Cargo manifest beside a 1.x tree: one dependency equal, one
    // differing, one missing; a package field equal; prose; a comment.
    let t = v1_full("cuts-report", "library-api");
    t.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nnom = \"8\"\n\n[dev-dependencies]\nproptest = \"1\"\n",
    );
    t.write(
        "specs/architecture.toml",
        "msrv = \"1.75\"\n\n[[features]]\nname = \"std\"\ndescription = \"Use std; never disable for the host build.\"\n\n[[external_deps]]\ncrate = \"serde\"\nversion = \"1\"\nfeatures = [\"derive\"]\n\n[[external_deps]]\ncrate = \"proptest\"\nversion = \"1\"\ntest_only = true\n\n[[external_deps]]\ncrate = \"rand\"\nversion = \"0.8\"\n",
    );
    t.write("spec.toml", &t.read("spec.toml").replacen("language = \"rust\"\n", "language = \"rust\"\nversion = \"0.1.0\"\n", 1));
    let out = text(&migrate(&t, &["--dry-run"]));
    let line = |needle: &str| out.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no line for {needle}:\n{out}")).to_string();
    assert!(line("crate = \"serde\"").contains("in manifest"), "{out}");
    assert!(line("crate = \"proptest\"").contains("in manifest"), "{out}");
    assert!(line("crate = \"nom\"").contains("differs"), "{out}");
    assert!(line("crate = \"rand\"").contains("not in manifest"), "{out}");
    assert!(line("[project].version").contains("in manifest"), "{out}");
    assert!(out.contains("never disable for the host build") && line("never disable").contains("prose"), "{out}");
    assert!(out.contains("# why nom: the parser"), "comments print:\n{out}");
}

#[test]
fn a_package_with_no_manifest_marks_every_removed_value_spec_only() {
    let t = v1_full("cuts-no-manifest", "library-api");
    let out = text(&migrate(&t, &["--dry-run"]));
    let l = out.lines().find(|l| l.contains("crate = \"nom\"")).unwrap_or_else(|| panic!("{out}"));
    assert!(l.contains("spec-only"), "{out}");
}

#[test]
fn a_workspace_dependency_and_a_rename_are_resolved() {
    let t = v1_full("cuts-ws-dep", "library-api");
    std::fs::create_dir_all(t.root.join("../ws")).unwrap();
    // A Cargo workspace above the package: `workspace = true` resolves there.
    t.write("../Cargo.toml", "[workspace]\nmembers = [\"proj\"]\n\n[workspace.dependencies]\nnom = { version = \"7\" }\n");
    t.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[dependencies]\nnom = { workspace = true }\nrenamed = { package = \"rand\", version = \"0.8\" }\n",
    );
    t.write("specs/architecture.toml", "[[external_deps]]\ncrate = \"rand\"\nversion = \"0.8\"\n");
    let out = text(&migrate(&t, &["--dry-run"]));
    let nom = out.lines().find(|l| l.contains("crate = \"nom\"")).unwrap_or_else(|| panic!("{out}"));
    assert!(nom.contains("in manifest"), "{out}");
    let rand = out.lines().find(|l| l.contains("crate = \"rand\"")).unwrap_or_else(|| panic!("{out}"));
    assert!(rand.contains("in manifest"), "{out}");
}

#[test]
fn a_dry_run_writes_nothing() {
    let t = v1_full("cuts-dry", "firmware-layered");
    let snap = |t: &Tree| {
        let mut v = Vec::new();
        fn walk(d: &Path, v: &mut Vec<(PathBuf, Vec<u8>)>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), v)
                } else {
                    v.push((e.path(), std::fs::read(e.path()).unwrap()))
                }
            }
        }
        walk(&t.root, &mut v);
        v.sort();
        v
    };
    let before = snap(&t);
    assert!(migrate(&t, &["--dry-run"]).status.success());
    assert!(before == snap(&t), "--dry-run wrote to disk");
}

#[test]
fn context_prints_context_files_and_decisions_keep_codegen_include() {
    let t = ci::tree("cuts-context", "library-api", &[]);
    t.write("docs/ref.md", "# reference\n");
    t.write(
        "specs/modules/Lib.toml",
        "name = \"Lib\"\nvisibility = \"pub\"\ndescription = \"x\"\ncontext_files = [\"docs/ref.md\"]\nguided_by = [\"DEC-001\"]\n",
    );
    t.write(
        "specs/decisions/DEC-001.toml",
        "[[decision]]\nid = \"DEC-001\"\ntitle = \"t\"\nstatus = \"accepted\"\ntext = \"x\"\nverification = \"check it\"\ncodegen_include = [\"text\", \"verification\"]\n",
    );
    let c = t.analyzer(&["context", "--module", "Lib"]);
    assert!(text(&c).contains("docs/ref.md"), "{}", text(&c));
    let d = t.analyzer(&["report", "decisions", "--module", "Lib"]);
    assert!(text(&d).contains("- verification"), "codegen_include still selects verification:\n{}", text(&d));
}

/// A crate that is both a normal and a dev dependency, with `workspace =
/// true` and member features (flxs1-core's postcard): the spec's dev entry
/// matches the manifest's dev entry, on the member's own features.
#[test]
fn a_dev_entry_matches_the_dev_dependency_on_its_own_features() {
    let t = v1_full("cuts-dev-entry", "library-api");
    t.write("../Cargo.toml", "[workspace]\nmembers = [\"proj\"]\n\n[workspace.dependencies]\npostcard = { version = \"1.1\", features = [\"experimental-derive\"] }\n");
    t.write(
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[dependencies]\npostcard = { workspace = true }\n\n[dev-dependencies]\npostcard = { workspace = true, features = [\"use-std\"] }\n",
    );
    t.write(
        "specs/architecture.toml",
        "[[external_deps]]\ncrate = \"postcard\"\nsource = \"workspace\"\nfeatures = [\"use-std\"]\ntest_only = true\n",
    );
    let out = text(&migrate(&t, &["--dry-run"]));
    let l = out.lines().find(|l| l.contains("crate = \"postcard\"")).unwrap_or_else(|| panic!("{out}"));
    assert!(l.contains("[in manifest]"), "{out}");
}

/// The K2 fields (an audio plugin's identity and bus constants, and its
/// panic strategy) have no reader and live in the plugin's code as
/// constants: 2.0 removes them, and the report lists each value.
#[test]
fn migrate_removes_the_audio_plugin_identity_and_bus_fields() {
    let text = "[plugin]\nname = \"p\"\nvendor = \"V\"\ncategory = \"fx\"\nformats = [\"clap\"]\nemail = \"a@b\"\nurl = \"https://x\"\nversion = \"0.1.0\"\n\n[audio]\ndefault_channel_layout = \"stereo\"\nmin_sample_rate = 44100\n\n[runtime]\npanic_handler = \"abort\"\nrealtime_audit = false\n";
    let (out, removed, keys_left) = specdev_analyzer::field_cuts::strip_architecture(text).unwrap();
    assert!(!keys_left, "nothing is left:\n{out}");
    let keys: Vec<&str> = removed.iter().map(|r| r.key.as_str()).collect();
    for k in ["plugin", "audio", "runtime.panic_handler", "runtime.realtime_audit"] {
        assert!(keys.iter().any(|x| *x == k || x.starts_with(&format!("{k}."))), "{k} not reported: {keys:?}");
    }
    let names: String = removed.iter().map(|r| r.value.clone()).collect::<Vec<_>>().join(" ");
    for v in ["\"V\"", "\"fx\"", "\"stereo\"", "\"abort\""] {
        assert!(names.contains(v), "value {v} not in the report: {names}");
    }
}
