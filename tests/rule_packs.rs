//! Rule packs: a block's or bridge's `checks.toml` and its query files.
//!
//! A unit ships its checks as data. `inline` copies `checks.toml` to
//! `specs/specdev/checks/{blocks,bridges}/<name>.toml` and every
//! `languages/<lang>/*.scm` to `specs/specdev/queries/{blocks,bridges}/<name>/<lang>/`.
//! `validate` runs the pack's graph checks; `check` runs its code checks.
//!
//! 1. `inline` writes the pack and its queries; they are in the snapshot and
//!    `drift` reports the unit clean.
//! 2. `migrate` writes the pack into a tree inlined before the unit had one.
//! 3. Severity order: the project's `domain.toml` > the pack's `[rules]` >
//!    `error`.
//! 4. A pack that fails its schema is an error naming the file.
//! 5. `unique` with `fields` keys on the composite value.
//! 6. `related_attrs_allowed` checks an attribute pair across a relation,
//!    and `"*"` matches any value.
//! 7. A code check compares the spec's entities with the code by key.
//! 8. `--merge` follows `drift` for a changed file set: an added file is
//!    written, a removed unedited file is deleted, a removed edited file
//!    refuses.
//! 9. `plugin-lint` refuses a pack that does not parse, a query that does
//!    not compile or lacks a mapped capture, an unknown primitive, and a
//!    rule name without the unit prefix.
//!
//! The test unit `tpack` is made here; no shipped unit is named.

use crate::ci;

use std::path::{Path, PathBuf};

use ci::{fw_module, plugin_with_block, text, tree_from, Tree};
use specdev_analyzer::{
    drift::{run_drift, FileCategory},
    layout::LayoutContext,
    merge::run_merge_block,
    upstream::{read_upstream_unit, UnitKind},
};

const SCHEMA: &str = r##"$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "https://specdev.dev/blocks/tpack.v1"
title: "tpack test block"
type: object
properties:
  tpack:
    type: object
    additionalProperties: false
    properties:
      planned: { type: boolean }
      parts:
        type: array
        items:
          type: object
          additionalProperties: false
          required: [name]
          properties:
            name: { type: string }
            bank: { type: string }
            slot: { type: integer }
            style: { type: string }
            planned: { type: boolean }
      knobs:
        type: array
        items:
          type: object
          additionalProperties: false
          required: [name, target]
          properties:
            name: { type: string }
            target: { type: string }
            mode: { type: string }
"##;

const REDUCTION: &str = r#"metaschema_version: "v1"
entities:
  - kind: Part
    extends: Module
    from: "specs/modules/*.toml"
    for_each: "tpack.parts[]"
    name: "${root.name}.${.name}"
    attrs:
      bank: ".bank"
      slot: ".slot"
      style: ".style"
  - kind: Knob
    extends: Module
    from: "specs/modules/*.toml"
    for_each: "tpack.knobs[]"
    name: "${root.name}.${.name}"
    attrs: {}
relations:
  - kind: knob_drives_part
    extends: depends_on
    from_kind: Knob
    to_kind: Part
    from_spec: "specs/modules/*.toml"
    for_each: "tpack.knobs[]"
    endpoints:
      from: "${root.name}.${.name}"
      to: "${root.name}.${.target}"
    attrs:
      mode: ".mode"
"#;

const CHECKS: &str = r#"[rules]
"tpack.parts_unique" = "warning"

[[graph_checks]]
kind = "unique"
rule = "tpack.parts_unique"
scope = "tpack.Part"
fields = ["attrs.bank", "attrs.slot"]

[[graph_checks]]
kind = "related_attrs_allowed"
rule = "tpack.knob_mode_fits"
relation_scope = "tpack.knob_drives_part"
left = "relation.mode"
right = "to.style"
allowed = [["fast", "sharp"], ["slow", "*"]]

[[code_checks]]
language = "rust"
query = "languages/rust/parts.scm"
entity_kind = "tpack.Part"
key_capture = "entity.key"
arg_key_capture = "arg.key"
arg_value_capture = "arg.value"

[code_checks.attributes]
slot = "slot"
"#;

/// One match per `key = value` argument of a `#[part(...)]` attribute,
/// with the name of the function it decorates.
const QUERY: &str = r#"(
  (attribute_item
    (attribute
      (identifier) @attr
      arguments: (token_tree
        (identifier) @arg.key
        .
        "="
        .
        [(identifier) (integer_literal)] @arg.value)))
  .
  (function_item
    name: (identifier) @entity.key)
  (#eq? @attr "part")
)
"#;

/// A copy of the test plugin with the `tpack` block; `with_pack` adds its
/// `checks.toml` and query file.
fn tpack_plugin(tag: &str, with_pack: bool) -> PathBuf {
    let plugin = plugin_with_block(tag, "tpack", SCHEMA, REDUCTION);
    let unit = plugin.join("blocks/tpack");
    std::fs::write(unit.join("guidance.md"), "# tpack — implementation guidance\n\n- A test rule.\n").unwrap();
    if with_pack {
        add_pack(&unit);
    }
    plugin
}

fn add_pack(unit: &Path) {
    std::fs::write(unit.join("checks.toml"), CHECKS).unwrap();
    std::fs::create_dir_all(unit.join("languages/rust")).unwrap();
    std::fs::write(unit.join("languages/rust/parts.scm"), QUERY).unwrap();
}

/// `drift` reports every unit file clean and no invariant. (The fixture's
/// profile files are edited by the 2.0 conversion, so the whole report is
/// not clean.)
fn units_clean(t: &Tree, plugin: &Path) {
    let report = run_drift(&layout(t), plugin).unwrap();
    let yaml = report.to_yaml().unwrap();
    assert!(report.invariants.is_empty(), "{yaml}");
    assert!(
        report.units.iter().all(|u| u.files.iter().all(|f| f.category == FileCategory::Clean)),
        "{yaml}"
    );
}

fn layout(t: &Tree) -> LayoutContext {
    LayoutContext::discover(&t.root, None).unwrap()
}

fn module(name: &str, body: &str) -> String {
    fw_module(name, "planned = true\n") + body
}

fn part(name: &str, bank: &str, slot: u8) -> String {
    format!("[[tpack.parts]]\nname = \"{name}\"\nbank = \"{bank}\"\nslot = {slot}\n")
}

/// Add `rule = "severity"` to the project's `[validation.rules]`.
fn project_severity(t: &Tree, rule: &str, severity: &str) {
    let d = t.read("specs/specdev/domain.toml");
    assert!(d.contains("[validation.rules]\n"), "{d}");
    let d = d.replacen("[validation.rules]\n", &format!("[validation.rules]\n\"{rule}\" = \"{severity}\"\n"), 1);
    t.write("specs/specdev/domain.toml", &d);
}

#[test]
fn inline_writes_the_pack_and_its_queries_into_the_snapshot() {
    let plugin = tpack_plugin("inline", true);
    let t = tree_from("inline", &plugin, "firmware-layered", "rust", &["tpack"]);
    assert_eq!(t.read("specs/specdev/checks/blocks/tpack.toml"), CHECKS);
    assert_eq!(t.read("specs/specdev/queries/blocks/tpack/rust/parts.scm"), QUERY);
    let snap = read_upstream_unit(&t.root.join("specs/specdev"), UnitKind::Block, "tpack").unwrap();
    assert!(snap.contains_key(Path::new("checks.toml")), "{:?}", snap.keys());
    assert!(snap.contains_key(Path::new("languages/rust/parts.scm")), "{:?}", snap.keys());
    units_clean(&t, &plugin);
}

#[test]
fn migrate_writes_the_pack_into_a_tree_inlined_before_it_existed() {
    let plugin = tpack_plugin("migrate", false);
    let t = tree_from("migrate", &plugin, "firmware-layered", "rust", &["tpack"]);
    assert!(!t.root.join("specs/specdev/checks").exists());
    add_pack(&plugin.join("blocks/tpack"));
    let out = t.analyzer(&["migrate"]);
    assert!(out.status.success(), "migrate:\n{}", text(&out));
    assert!(t.root.join("specs/specdev/checks/blocks/tpack.toml").is_file(), "{}", text(&out));
    assert!(t.root.join("specs/specdev/queries/blocks/tpack/rust/parts.scm").is_file(), "{}", text(&out));
    let report = run_drift(&layout(&t), &plugin).unwrap();
    assert!(report.invariants.is_empty(), "no invariant: {:?}", report.invariants);
}

#[test]
fn unique_fields_reports_a_duplicate_composite_key_only() {
    let plugin = tpack_plugin("unique", true);
    let t = tree_from("unique", &plugin, "firmware-layered", "rust", &["tpack"]);
    t.write("specs/modules/Alpha.toml", &module("Alpha", &(part("a1", "A", 1) + &part("a2", "A", 2))))
        // Same bank and slot as Alpha.a1: a duplicate.
        .write("specs/modules/Beta.toml", &module("Beta", &(part("b1", "A", 1) + &part("b2", "B", 2))));
    let c = t.check(&[]);
    let hits: Vec<_> = c.ran().iter().filter(|f| f.rule == "tpack.parts_unique").collect();
    assert_eq!(hits.len(), 1, "one duplicate (A, 1); A2 and B2 differ in one part:\n{}", c.raw);
    // The pack's [rules] makes it a warning.
    assert_eq!(hits[0].severity, "warning", "{}", c.raw);
    assert!(hits[0].entity == "Beta.b1" || hits[0].entity == "Alpha.a1", "{:?}", hits[0]);
}

#[test]
fn the_project_severity_overrides_the_pack_severity() {
    let plugin = tpack_plugin("severity", true);
    let t = tree_from("severity", &plugin, "firmware-layered", "rust", &["tpack"]);
    t.write("specs/modules/Alpha.toml", &module("Alpha", &(part("a1", "A", 1) + &part("a2", "A", 1))));
    project_severity(&t, "tpack.parts_unique", "info");
    let c = t.check(&[]);
    let hits: Vec<_> = c.ran().iter().filter(|f| f.rule == "tpack.parts_unique").collect();
    assert_eq!(hits.len(), 1, "{}", c.raw);
    assert_eq!(hits[0].severity, "info", "{}", c.raw);
}

#[test]
fn a_rule_no_one_sets_is_an_error() {
    let plugin = tpack_plugin("default-sev", true);
    let t = tree_from("default-sev", &plugin, "firmware-layered", "rust", &["tpack"]);
    // A knob whose mode does not fit the part's style: no [rules] entry.
    t.write(
        "specs/modules/Alpha.toml",
        &module(
            "Alpha",
            &("[[tpack.parts]]\nname = \"p\"\nstyle = \"round\"\n".to_string()
                + "[[tpack.knobs]]\nname = \"k\"\ntarget = \"p\"\nmode = \"fast\"\n"),
        ),
    );
    t.check(&[]).fires("tpack.knob_mode_fits", "Alpha.k -> Alpha.p", "error");
}

#[test]
fn related_attrs_allowed_checks_the_pair_and_accepts_a_wildcard() {
    let plugin = tpack_plugin("related", true);
    let t = tree_from("related", &plugin, "firmware-layered", "rust", &["tpack"]);
    let parts = "[[tpack.parts]]\nname = \"sharp\"\nstyle = \"sharp\"\n\n[[tpack.parts]]\nname = \"round\"\nstyle = \"round\"\n\n[[tpack.parts]]\nname = \"plain\"\n\n";
    let knob = |n: &str, target: &str, mode: &str| {
        format!("[[tpack.knobs]]\nname = \"{n}\"\ntarget = \"{target}\"\nmode = \"{mode}\"\n\n")
    };
    t.write(
        "specs/modules/Alpha.toml",
        &module(
            "Alpha",
            &format!(
                "{parts}{}{}{}{}{}",
                knob("ok", "sharp", "fast"),
                knob("bad", "round", "fast"),
                knob("wild", "round", "slow"),
                knob("unlisted", "sharp", "medium"),
                // The part has no style: skipped (the schema owns presence).
                knob("nostyle", "plain", "fast"),
            ),
        ),
    );
    let c = t.check(&[]);
    c.silent("tpack.knob_mode_fits", "Alpha.ok -> Alpha.sharp")
        .fires("tpack.knob_mode_fits", "Alpha.bad -> Alpha.round", "error")
        .silent("tpack.knob_mode_fits", "Alpha.wild -> Alpha.round")
        .fires("tpack.knob_mode_fits", "Alpha.unlisted -> Alpha.sharp", "error")
        .silent("tpack.knob_mode_fits", "Alpha.nostyle -> Alpha.plain");
}

#[test]
fn a_pack_that_fails_its_schema_is_an_error_naming_the_file() {
    let plugin = tpack_plugin("bad-pack", true);
    let t = tree_from("bad-pack", &plugin, "firmware-layered", "rust", &["tpack"]);
    t.write("specs/specdev/checks/blocks/tpack.toml", "[[graph_checks]]\nkind = 5\n");
    let c = t.check(&[]);
    let hits: Vec<_> = c.ran().iter().filter(|f| f.rule == "PACK-001").collect();
    assert_eq!(hits.len(), 1, "{}", c.raw);
    assert_eq!(hits[0].severity, "error");
    assert!(c.raw.contains("checks/blocks/tpack.toml"), "the finding names the file:\n{}", c.raw);
    assert_eq!(c.exit, Some(1));
}

const PARTS_SRC: &str = r#"#[part(slot = 1)]
fn found_ok() {}
#[part(slot = 9)]
fn found_bad() {}
"#;

#[test]
fn a_code_check_compares_spec_entities_with_the_code_by_key() {
    let plugin = tpack_plugin("code", true);
    let t = tree_from("code", &plugin, "firmware-layered", "rust", &["tpack"]);
    let src = "source_file = \"src/parts.rs\"\n";
    t.write("src/parts.rs", PARTS_SRC).write(
        "specs/modules/Alpha.toml",
        &(fw_module("Alpha", src)
            + &part("found_ok", "A", 1)
            + &part("found_bad", "A", 2)
            + &part("missing", "A", 3)),
    );
    let c = t.check(&[]);
    c.silent("pack_attribute_matches", "Alpha.found_ok")
        .silent("pack_entity_in_code", "Alpha.found_ok")
        .fires("pack_attribute_matches", "Alpha.found_bad", "error")
        .fires("pack_entity_in_code", "Alpha.missing", "error");
}

// ---------------------------------------------------------------------------
// --merge with a changed file set (F2).
// ---------------------------------------------------------------------------

#[test]
fn merge_writes_a_file_the_plugin_added() {
    let plugin = tpack_plugin("merge-add", false);
    let t = tree_from("merge-add", &plugin, "firmware-layered", "rust", &["tpack"]);
    add_pack(&plugin.join("blocks/tpack"));
    run_merge_block(&layout(&t), &plugin, "tpack").expect("merge carries an added file");
    assert_eq!(t.read("specs/specdev/checks/blocks/tpack.toml"), CHECKS);
    assert_eq!(t.read("specs/specdev/queries/blocks/tpack/rust/parts.scm"), QUERY);
    let snap = read_upstream_unit(&t.root.join("specs/specdev"), UnitKind::Block, "tpack").unwrap();
    assert!(snap.contains_key(Path::new("checks.toml")), "{:?}", snap.keys());
    units_clean(&t, &plugin);
}

#[test]
fn merge_deletes_a_removed_file_the_project_did_not_edit() {
    let plugin = tpack_plugin("merge-rm", true);
    let t = tree_from("merge-rm", &plugin, "firmware-layered", "rust", &["tpack"]);
    std::fs::remove_dir_all(plugin.join("blocks/tpack/languages")).unwrap();
    run_merge_block(&layout(&t), &plugin, "tpack").expect("merge carries a removed file");
    assert!(!t.root.join("specs/specdev/queries/blocks/tpack/rust/parts.scm").exists());
    let snap = read_upstream_unit(&t.root.join("specs/specdev"), UnitKind::Block, "tpack").unwrap();
    assert!(!snap.contains_key(Path::new("languages/rust/parts.scm")), "{:?}", snap.keys());
    units_clean(&t, &plugin);
}

#[test]
fn merge_refuses_a_removed_file_the_project_edited() {
    let plugin = tpack_plugin("merge-edited", true);
    let t = tree_from("merge-edited", &plugin, "firmware-layered", "rust", &["tpack"]);
    let q = "specs/specdev/queries/blocks/tpack/rust/parts.scm";
    t.write(q, &format!(";; a local note\n{QUERY}"));
    std::fs::remove_dir_all(plugin.join("blocks/tpack/languages")).unwrap();
    let err = run_merge_block(&layout(&t), &plugin, "tpack").expect_err("an edited removed file refuses");
    let msg = format!("{err:#}");
    assert!(msg.contains("languages/rust/parts.scm"), "names the file: {msg}");
    assert!(t.root.join(q).is_file(), "the edited copy stays");
}

// ---------------------------------------------------------------------------
// plugin-lint.
// ---------------------------------------------------------------------------

fn plugin_lint(plugin: &Path) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .args(["plugin-lint", "--plugin"])
        .arg(plugin)
        .arg("--no-color")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stderr).to_string()
}

fn tpack_lines(out: &str, code: &str) -> Vec<String> {
    out.lines().filter(|l| l.starts_with("tpack:") && l.contains(code)).map(str::to_string).collect()
}

#[test]
fn plugin_lint_accepts_a_good_pack() {
    let plugin = tpack_plugin("lint-ok", true);
    let out = plugin_lint(&plugin);
    let hits: Vec<&str> = out.lines().filter(|l| l.starts_with("tpack:")).collect();
    assert!(hits.is_empty(), "{out}");
}

#[test]
fn plugin_lint_refuses_a_pack_that_fails_its_schema() {
    let plugin = tpack_plugin("lint-schema", true);
    std::fs::write(plugin.join("blocks/tpack/checks.toml"), "[[graph_checks]]\nkind = 5\n").unwrap();
    assert_eq!(tpack_lines(&plugin_lint(&plugin), "[PACK-LINT-001]").len(), 1);
}

#[test]
fn plugin_lint_refuses_a_query_that_does_not_compile_or_lacks_a_capture() {
    let plugin = tpack_plugin("lint-query", true);
    let q = plugin.join("blocks/tpack/languages/rust/parts.scm");
    std::fs::write(&q, "(function_item name: (identifier) @entity.key\n").unwrap();
    assert_eq!(tpack_lines(&plugin_lint(&plugin), "[PACK-LINT-002]").len(), 1, "does not compile");
    std::fs::write(&q, "(function_item name: (identifier) @entity.key)\n").unwrap();
    let out = plugin_lint(&plugin);
    let hits = tpack_lines(&out, "[PACK-LINT-003]");
    assert_eq!(hits.len(), 1, "one line names the missing captures:\n{out}");
    assert!(hits[0].contains("arg.key"), "{out}");
}

#[test]
fn plugin_lint_refuses_an_unknown_primitive_and_an_unprefixed_rule() {
    let plugin = tpack_plugin("lint-rules", true);
    let checks = CHECKS
        .replace("kind = \"unique\"", "kind = \"unique_ish\"")
        .replace("rule = \"tpack.knob_mode_fits\"", "rule = \"knob_mode_fits\"");
    std::fs::write(plugin.join("blocks/tpack/checks.toml"), checks).unwrap();
    let out = plugin_lint(&plugin);
    assert_eq!(tpack_lines(&out, "[PACK-LINT-004]").len(), 1, "unknown primitive:\n{out}");
    assert_eq!(tpack_lines(&out, "[PACK-LINT-005]").len(), 1, "unprefixed rule:\n{out}");
}

// ---------------------------------------------------------------------------
// Review findings (2026-10-05).
// ---------------------------------------------------------------------------

#[test]
fn a_key_found_at_two_code_locations_is_ambiguous_not_merged() {
    let plugin = tpack_plugin("ambiguous", true);
    let t = tree_from("ambiguous", &plugin, "firmware-layered", "rust", &["tpack"]);
    t.write("src/parts.rs", "#[part(slot = 1)]\nfn twice() {}\n")
        .write("src/more.rs", "#[part(slot = 5)]\nfn twice() {}\n")
        .write(
            "specs/modules/Alpha.toml",
            &(fw_module("Alpha", "source_file = \"src/parts.rs\"\n") + &part("twice", "A", 1)),
        );
    let c = t.check(&[]);
    c.fires("pack_key_ambiguous", "Alpha.twice", "error")
        .silent("pack_attribute_matches", "Alpha.twice")
        .silent("pack_entity_in_code", "Alpha.twice");
}

#[test]
fn an_inlined_query_that_does_not_compile_is_pack_002() {
    let plugin = tpack_plugin("pack-002", true);
    let t = tree_from("pack-002", &plugin, "firmware-layered", "rust", &["tpack"]);
    t.write("src/parts.rs", PARTS_SRC)
        .write("specs/modules/Alpha.toml", &(fw_module("Alpha", "source_file = \"src/parts.rs\"\n") + &part("found_ok", "A", 1)))
        .write("specs/specdev/queries/blocks/tpack/rust/parts.scm", "(function_item name: (identifier) @entity.key\n");
    let c = t.check(&[]);
    let hits: Vec<_> = c.ran().iter().filter(|f| f.rule == "PACK-002").collect();
    assert_eq!(hits.len(), 1, "{}", c.raw);
    assert_eq!(hits[0].severity, "error");
    assert_eq!(c.exit, Some(1));
}

#[test]
fn a_graph_check_missing_its_arguments_fails_the_schema() {
    for (tag, from, to) in [
        ("no-fields", "fields = [\"attrs.bank\", \"attrs.slot\"]\n", ""),
        ("no-allowed", "allowed = [[\"fast\", \"sharp\"], [\"slow\", \"*\"]]\n", ""),
    ] {
        let plugin = tpack_plugin(&format!("lint-{tag}"), true);
        let checks = CHECKS.replace(from, to);
        assert_ne!(checks, CHECKS, "{tag}");
        std::fs::write(plugin.join("blocks/tpack/checks.toml"), &checks).unwrap();
        assert_eq!(tpack_lines(&plugin_lint(&plugin), "[PACK-LINT-001]").len(), 1, "{tag}");
        let t = tree_from(tag, &tpack_plugin(&format!("run-{tag}"), true), "firmware-layered", "rust", &["tpack"]);
        t.write("specs/specdev/checks/blocks/tpack.toml", &checks);
        let c = t.check(&[]);
        assert!(c.ran().iter().any(|f| f.rule == "PACK-001" && f.severity == "error"), "{tag}:\n{}", c.raw);
    }
}

#[test]
fn the_primitive_list_names_every_dispatch_arm() {
    let src = include_str!("../src/validate.rs");
    let start = src.find("fn execute_graph_checks(").unwrap();
    let end = start + src[start..].find("\n}\n").unwrap();
    let arms: std::collections::BTreeSet<&str> = src[start..end]
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix('"')?;
            let (name, tail) = rest.split_once('"')?;
            tail.trim_start().starts_with("=>").then_some(name)
        })
        .collect();
    let listed: std::collections::BTreeSet<&str> =
        specdev_analyzer::validate::GRAPH_CHECK_PRIMITIVES.iter().copied().collect();
    assert_eq!(arms, listed);
}

#[test]
fn merge_deletes_a_removed_file_whose_copy_differs_only_in_line_endings() {
    let plugin = tpack_plugin("merge-crlf", true);
    let t = tree_from("merge-crlf", &plugin, "firmware-layered", "rust", &["tpack"]);
    let q = "specs/specdev/queries/blocks/tpack/rust/parts.scm";
    t.write(q, &QUERY.replace('\n', "\r\n"));
    std::fs::remove_dir_all(plugin.join("blocks/tpack/languages")).unwrap();
    run_merge_block(&layout(&t), &plugin, "tpack").expect("a line-ending difference is not an edit");
    assert!(!t.root.join(q).exists());
    units_clean(&t, &plugin);
}

// ---------------------------------------------------------------------------
// `arg_key`: a fixed argument name for a value with no key node.
// ---------------------------------------------------------------------------

const STYLE_QUERY: &str = r#"(
  (attribute_item (attribute (identifier) @style))
  .
  (function_item name: (identifier) @entity.key)
  (#any-of? @style "round" "sharp")
)
"#;

const STYLE_CHECK: &str = r#"
[[code_checks]]
language = "rust"
query = "languages/rust/styles.scm"
entity_kind = "tpack.Part"
key_capture = "entity.key"
arg_key = "style"
arg_value_capture = "style"

[code_checks.attributes]
style = "style"
"#;

fn style_plugin(tag: &str, checks: &str) -> PathBuf {
    let plugin = tpack_plugin(tag, false);
    let unit = plugin.join("blocks/tpack");
    std::fs::write(unit.join("checks.toml"), checks).unwrap();
    std::fs::create_dir_all(unit.join("languages/rust")).unwrap();
    std::fs::write(unit.join("languages/rust/styles.scm"), STYLE_QUERY).unwrap();
    plugin
}

#[test]
fn arg_key_names_the_value_of_a_capture_with_no_key_node() {
    let plugin = style_plugin("arg-key", STYLE_CHECK);
    let t = tree_from("arg-key", &plugin, "firmware-layered", "rust", &["tpack"]);
    let styled = |n: &str, style: &str| format!("[[tpack.parts]]\nname = \"{n}\"\nstyle = \"{style}\"\n\n");
    t.write("src/parts.rs", "#[round]\nfn same() {}\n#[sharp]\nfn differs() {}\n").write(
        "specs/modules/Alpha.toml",
        &(fw_module("Alpha", "source_file = \"src/parts.rs\"\n") + &styled("same", "round") + &styled("differs", "round")),
    );
    t.check(&[])
        .silent("pack_attribute_matches", "Alpha.same")
        .silent("pack_entity_in_code", "Alpha.same")
        .fires("pack_attribute_matches", "Alpha.differs", "error");
}

#[test]
fn plugin_lint_refuses_arg_key_with_arg_key_capture() {
    let both = STYLE_CHECK.replace("arg_key = \"style\"", "arg_key = \"style\"\narg_key_capture = \"style\"");
    let plugin = style_plugin("arg-key-both", &both);
    assert_eq!(tpack_lines(&plugin_lint(&plugin), "[PACK-LINT-001]").len(), 1);
    let good = style_plugin("arg-key-good", STYLE_CHECK);
    let out = plugin_lint(&good);
    assert!(out.lines().all(|l| !l.starts_with("tpack:")), "{out}");
}
