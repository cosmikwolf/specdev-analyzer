//! The checks against code: test discovery and requirement ↔ test,
//! internal-edge imports, the open-question gate, `--since`, and the
//! ids-in-document check.
//!
//! These tests state the target before the code exists. What each test
//! holds fixed:
//!
//! 1. Test discovery is its own pass: tests in nested `mod`s, in `tests/`,
//!    in a `[paths].tests` root, and under async / parameterized test
//!    attributes all resolve; nothing outside the roots does.
//! 2. Identity: a bare name when unique; `<Module>::<name>` when two
//!    modules share it; `<file path>::<name>` for a test in no module's
//!    source. A nameless form (`proptest!`) is named by its enclosing
//!    module.
//! 3. `verified_by` resolves on every entity kind that carries it, not
//!    only on requirements.
//! 4. Per-language discovery (Rust, TypeScript, Dart, C, C++): the test
//!    forms, the identity, and the pending idiom.
//! 5. Internal edges: a sibling workspace crate is not an edge; modules
//!    that share one file are one node and their edge is reported skipped;
//!    TypeScript path aliases and barrel files resolve to the module.
//! 6. The open-question gate reads workspace-tier questions too.
//! 7. `check --since <ref>` reports every new flag, every `hard_gate`
//!    flip and every default committed on a `hard_gate = false` question.
//! 8. The ids-in-document check is configured in `domain.toml`.

use crate::ci;

use ci::{fw_module, plugin_with_block, requirement, tree, tree_from, tree_lang, Tree};

fn verified(t: &Tree, id: &str, names: &[&str]) {
    let list = names
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    t.write(
        &format!("specs/requirements/{id}.toml"),
        &requirement(id, &format!("verified_by = [{list}]\n")),
    );
}

/// A firmware tree with one built module `BuiltOk` that satisfies `reqs`.
fn built(tag: &str, reqs: &[&str]) -> Tree {
    let t = tree(tag, "firmware-layered", &[]);
    let sat = reqs
        .iter()
        .map(|r| format!("\"{r}\""))
        .collect::<Vec<_>>()
        .join(", ");
    t.write(
        "specs/modules/BuiltOk.toml",
        &fw_module("BuiltOk", &format!("satisfies = [{sat}]\n")),
    );
    t
}

const RUST_TESTS: &str = r#"pub fn built() {}

#[cfg(test)]
mod tests {
    mod inner {
        #[test]
        fn nested_mod_test() {}
    }
    #[tokio::test]
    async fn async_case() {}
    #[rstest]
    fn param_case() {}
}

#[cfg(test)]
mod props {
    proptest::proptest! {
        #[test]
        fn roundtrip(x in 0u8..10) { let _ = x; }
    }
}
"#;

#[test]
fn rust_test_forms_and_roots_resolve() {
    let t = built("rust-forms", &["REQ-T-001", "REQ-T-002"]);
    t.write("src/built_ok.rs", RUST_TESTS)
        .write(
            "tests/integration.rs",
            "#[test]\nfn integration_case() {}\n",
        )
        .write("bench/tests/bench.rs", "#[test]\nfn bench_case() {}\n")
        .write("elsewhere/stray.rs", "#[test]\nfn stray_case() {}\n");
    let spec = t.read("spec.toml");
    t.write(
        "spec.toml",
        &spec.replacen("[paths]\n", "[paths]\ntests = [\"bench/tests\"]\n", 1),
    );
    verified(
        &t,
        "REQ-T-001",
        &[
            "nested_mod_test",
            "async_case",
            "param_case",
            "integration_case",
            "bench_case",
            "props",
        ],
    );
    verified(&t, "REQ-T-002", &["stray_case"]);
    let c = t.check(&[]);
    c.silent("verified_by_resolves", "REQ-T-001")
        // Outside every test root: not discovered.
        .fires("verified_by_resolves", "REQ-T-002", "error");
}

#[test]
fn a_paths_tests_root_is_needed_for_tests_outside_the_defaults() {
    let t = built("rust-no-root", &["REQ-T-003"]);
    t.write("src/built_ok.rs", "pub fn built() {}\n")
        .write("bench/tests/bench.rs", "#[test]\nfn bench_case() {}\n");
    verified(&t, "REQ-T-003", &["bench_case"]);
    t.check(&[])
        .fires("verified_by_resolves", "REQ-T-003", "error");
}

#[test]
fn rust_test_identity() {
    let t = built(
        "rust-identity",
        &["REQ-I-001", "REQ-I-002", "REQ-I-003", "REQ-I-004"],
    );
    t.write("specs/modules/Other.toml", &fw_module("Other", ""))
        .write(
            "src/built_ok.rs",
            "#[cfg(test)]\nmod t {\n    #[test]\n    fn dup() {}\n}\n",
        )
        .write(
            "src/other.rs",
            "#[cfg(test)]\nmod t {\n    #[test]\n    fn dup() {}\n}\n",
        )
        .write("tests/a.rs", "#[test]\nfn dup2() {}\n")
        .write("tests/b.rs", "#[test]\nfn dup2() {}\n");
    verified(&t, "REQ-I-001", &["dup"]); // ambiguous bare name
    verified(&t, "REQ-I-002", &["BuiltOk::dup", "Other::dup"]);
    verified(&t, "REQ-I-003", &["dup2"]); // ambiguous bare name
    verified(&t, "REQ-I-004", &["tests/a.rs::dup2"]);
    t.check(&[])
        .fires("verified_by_resolves", "REQ-I-001", "error")
        .silent("verified_by_resolves", "REQ-I-002")
        .fires("verified_by_resolves", "REQ-I-003", "error")
        .silent("verified_by_resolves", "REQ-I-004");
}

const UI_SCHEMA: &str = r#"$schema: "https://json-schema.org/draft/2020-12/schema"
$id: "https://specdev.dev/blocks/uitest.v1"
title: "uitest block (test only)"
type: object
properties:
  uitest:
    type: object
    additionalProperties: false
    properties:
      interactions:
        type: array
        items:
          type: object
          additionalProperties: false
          required: [name]
          properties:
            name: { type: string }
            verified_by: { type: array, items: { type: string } }
"#;

const UI_REDUCTION: &str = r#"metaschema_version: "v1"
entities:
  - kind: Interaction
    extends: Module
    from: "specs/modules/*.toml"
    for_each: "uitest.interactions[]"
    name: "${root.name}.${.name}"
    attrs:
      verified_by: ".verified_by"
"#;

#[test]
fn verified_by_resolves_on_every_entity_kind() {
    let plugin = plugin_with_block("every-kind", "uitest", UI_SCHEMA, UI_REDUCTION);
    let t = tree_from(
        "every-kind",
        &plugin,
        "firmware-layered",
        "rust",
        &["uitest"],
    );
    t.write(
        "specs/modules/Screen.toml",
        &(fw_module("Screen", "")
            + "\n[[uitest.interactions]]\nname = \"tap\"\nverified_by = [\"tap_test\"]\n\n[[uitest.interactions]]\nname = \"swipe\"\nverified_by = [\"absent_swipe_test\"]\n"),
    )
    .write("src/screen.rs", "#[cfg(test)]\nmod t {\n    #[test]\n    fn tap_test() {}\n}\n");
    t.check(&[])
        .silent("verified_by_resolves", "Screen.tap")
        .fires("verified_by_resolves", "Screen.swipe", "error");
}

// ---- Per-language discovery: `report tests --language L --root R --format yaml`.

struct Found {
    identity: String,
    pending: bool,
}

fn discover(t: &Tree, language: &str, root: &str) -> Vec<Found> {
    let out = t.analyzer(&[
        "report",
        "tests",
        "--language",
        language,
        "--root",
        root,
        "--format",
        "yaml",
    ]);
    let v: serde_json::Value = serde_yaml::from_slice(&out.stdout)
        .unwrap_or_else(|_| panic!("`report tests` did not run and report:\n{}", ci::text(&out)));
    v["tests"]
        .as_array()
        .unwrap_or_else(|| panic!("no `tests` array:\n{}", ci::text(&out)))
        .iter()
        .map(|f| Found {
            identity: f["identity"].as_str().unwrap_or_default().to_string(),
            pending: f["pending"].as_bool().unwrap_or(false),
        })
        .collect()
}

#[track_caller]
fn has(found: &[Found], identity: &str, pending: bool) {
    assert!(
        found
            .iter()
            .any(|f| f.identity == identity && f.pending == pending),
        "expected `{identity}` (pending = {pending}); discovered: {:?}",
        found
            .iter()
            .map(|f| (&f.identity, f.pending))
            .collect::<Vec<_>>()
    );
}

#[track_caller]
fn lacks(found: &[Found], identity: &str) {
    assert!(
        !found.iter().any(|f| f.identity == identity),
        "`{identity}` must not be discovered"
    );
}

#[test]
fn typescript_discovery() {
    let t = tree("ts-disc", "library-api", &[]);
    t.write(
        "web/cart.test.ts",
        "describe('cart', () => {\n  it('adds', () => {});\n  describe('empty', () => { it('adds', () => {}); });\n  it.skip('removes', () => {});\n  it.todo('merges');\n});\ntest('unique title', () => {});\n",
    )
    .write("web/cart.ts", "export function notATest() { it('inside source', () => {}); }\n");
    let f = discover(&t, "typescript", "web");
    has(&f, "unique title", false);
    has(&f, "cart > adds", false); // title not unique
    has(&f, "cart > empty > adds", false);
    has(&f, "removes", true);
    has(&f, "merges", true);
    lacks(&f, "inside source"); // not a test file
}

#[test]
fn dart_discovery() {
    let t = tree("dart-disc", "library-api", &[]);
    t.write(
        "app/test/widget_test.dart",
        "void main() {\n  group('cart', () {\n    test('adds', () {});\n    testWidgets('renders', (tester) async {});\n    test('skipped', () {}, skip: 'flaky');\n  });\n  test('adds', () {});\n}\n",
    );
    let f = discover(&t, "dart", "app/test");
    has(&f, "cart > adds", false);
    has(&f, "renders", false);
    has(&f, "skipped", true);
}

#[test]
fn cpp_discovery() {
    let t = tree("cpp-disc", "library-api", &[]);
    t.write(
        "native/tests/math_test.cpp",
        "TEST(Math, Adds) {}\nTEST_F(Fixture, Uses) {}\nTEST(Math, DISABLED_Slow) {}\nTEST_CASE(\"catch title\") {}\nTEST_CASE(\"hidden title\", \"[.]\") {}\n",
    );
    let f = discover(&t, "cpp", "native/tests");
    has(&f, "Math.Adds", false);
    has(&f, "Fixture.Uses", false);
    has(&f, "Math.DISABLED_Slow", true);
    has(&f, "catch title", false);
    has(&f, "hidden title", true);
}

#[test]
fn c_discovery() {
    let t = tree("c-disc", "library-api", &[]);
    t.write(
        "native/tests/test_ring.c",
        "void test_push(void) {}\nvoid test_skip(void) { TEST_IGNORE(); }\nvoid helper(void) {}\nint main(void) { RUN_TEST(test_push); RUN_TEST(test_skip); return 0; }\n",
    );
    let f = discover(&t, "c", "native/tests");
    has(&f, "test_push", false);
    has(&f, "test_skip", true);
    lacks(&f, "helper");
}

#[test]
fn rust_discovery_reports_pending() {
    let t = tree("rust-disc", "library-api", &[]);
    t.write("src/lib.rs", "#[cfg(test)]\nmod t {\n    #[test]\n    fn live() {}\n    #[test]\n    #[ignore]\n    fn off() {}\n}\n");
    let f = discover(&t, "rust", "src");
    has(&f, "live", false);
    has(&f, "off", true);
}

// ---- Internal edges.

#[test]
fn internal_edges_scope_and_shared_files() {
    let t = tree("edges-scope", "firmware-layered", &[]);
    t.write("specs/modules/Uses.toml", &fw_module("Uses", ""))
        .write(
            "src/uses.rs",
            "use sibling_crate::thing;\nuse core::fmt;\npub fn f() {}\n",
        )
        .write(
            "specs/modules/Ticker.toml",
            &fw_module("Ticker", "source_file = \"src/main.rs\"\n"),
        )
        .write(
            "specs/modules/Blinky.toml",
            &fw_module("Blinky", "source_file = \"src/main.rs\"\n"),
        )
        .write(
            "src/main.rs",
            "mod x { pub fn a() {} }\nuse crate::x::a;\nfn main() { a() }\n",
        )
        .write(
            "specs/modules/Undeclared.toml",
            &fw_module("Undeclared", ""),
        )
        .write(
            "src/undeclared.rs",
            "use crate::uses::f;\npub fn g() { f() }\n",
        )
        .write(
            "specs/modules/Root.toml",
            &fw_module("Root", "source_file = \"src/lib.rs\"\n"),
        )
        .write(
            "src/lib.rs",
            "mod uses;\nmod undeclared;\nuse uses::f;\npub fn h() { f() }\n",
        );
    t.check(&[])
        // A sibling workspace crate and `core` are external: the manifest owns them.
        .silent("internal_edges_declared", "Uses")
        // Two modules in one file: one node; each is told its edges are not checked.
        .fires("internal_edges_skipped", "Ticker", "info")
        .fires("internal_edges_skipped", "Blinky", "info")
        // A crate-root file may name a sibling module without `crate::`.
        .fires("internal_edges_declared", "Root", "error")
        .fires("internal_edges_declared", "Undeclared", "error");
}

fn ts_service(name: &str, file: &str, extra: &str) -> String {
    format!("name = \"{name}\"\nrole = \"service\"\nenvironment = \"server\"\ndescription = \"Fixture.\"\nsource_file = \"{file}\"\n{extra}")
}

#[test]
fn internal_edges_resolve_typescript_aliases_and_barrels() {
    let t = tree_lang("edges-ts", "nextjs-app", "typescript");
    t.write(
        "specs/modules/App.toml",
        "name = \"App\"\nrole = \"app\"\ndescription = \"Root.\"\n",
    )
    .write(
        "app/layout.tsx",
        "export default function RootLayout() { return null; }\n",
    );
    t.write(
        "tsconfig.json",
        "{ \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"src/*\"] } } }\n",
    )
    .write("src/lib/b.ts", "export const b = 1;\n")
    .write("src/lib/index.ts", "export * from './b';\n")
    .write(
        "src/viaalias.ts",
        "import { b } from '@/lib/b';\nexport const a = b;\n",
    )
    .write(
        "src/viabarrel.ts",
        "import { b } from '@/lib';\nexport const c = b;\n",
    )
    .write(
        "src/declared.ts",
        "import { b } from '@/lib';\nexport const d = b;\n",
    )
    .write("specs/modules/B.toml", &ts_service("B", "src/lib/b.ts", ""))
    .write(
        "specs/modules/ViaAlias.toml",
        &ts_service("ViaAlias", "src/viaalias.ts", ""),
    )
    .write(
        "specs/modules/ViaBarrel.toml",
        &ts_service("ViaBarrel", "src/viabarrel.ts", ""),
    )
    .write(
        "specs/modules/Declared.toml",
        &ts_service("Declared", "src/declared.ts", "depends_on = [\"B\"]\n"),
    );
    t.check(&[])
        .fires("internal_edges_declared", "ViaAlias", "error")
        .fires("internal_edges_declared", "ViaBarrel", "error")
        .silent("internal_edges_declared", "Declared");
}

#[test]
fn internal_edges_follow_rust_super_paths() {
    let t = tree("edges-super", "firmware-layered", &[]);
    t.write("specs/modules/Front.toml", &fw_module("Front", ""))
        .write(
            "src/front.rs",
            "use super::back::run;\npub fn go() { run() }\n",
        )
        .write("specs/modules/Back.toml", &fw_module("Back", ""))
        .write("src/back.rs", "pub fn run() {}\n")
        .write(
            "specs/modules/Front2.toml",
            &fw_module("Front2", "depends_on = [\"Back\"]\n"),
        )
        .write(
            "src/front2.rs",
            "use super::back::run;\npub fn go() { run() }\n",
        );
    t.check(&[])
        .fires("internal_edges_declared", "Front", "error")
        .silent("internal_edges_declared", "Front2")
        .silent("internal_edges_declared", "Back");
}

#[test]
fn internal_edges_follow_rust_qualified_paths_without_use() {
    // #238: a `crate::` / `super::` / `self::` path in an expression or a
    // type is a dependency even with no `use` line for it.
    let t = tree("edges-qualified", "firmware-layered", &[]);
    t.write("specs/modules/Led.toml", &fw_module("Led", ""))
        .write(
            "src/led.rs",
            "pub const DEFAULT_BRIGHTNESS: u8 = 8;\npub struct Led;\npub fn on() {}\n",
        )
        .write("specs/modules/ByValue.toml", &fw_module("ByValue", ""))
        .write(
            "src/by_value.rs",
            "pub fn f() -> u8 { crate::led::DEFAULT_BRIGHTNESS }\n",
        )
        .write("specs/modules/ByType.toml", &fw_module("ByType", ""))
        .write(
            "src/by_type.rs",
            "pub fn f(_l: &crate::led::Led) {}\n",
        )
        .write("specs/modules/BySuper.toml", &fw_module("BySuper", ""))
        .write("src/by_super.rs", "pub fn f() { super::led::on() }\n")
        .write(
            "specs/modules/Declared.toml",
            &fw_module("Declared", "depends_on = [\"Led\"]\n"),
        )
        .write(
            "src/declared.rs",
            "pub fn f() -> u8 { crate::led::DEFAULT_BRIGHTNESS }\n",
        )
        .write("specs/modules/TestOnly.toml", &fw_module("TestOnly", ""))
        .write(
            "src/test_only.rs",
            "pub fn f() {}\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() { crate::led::on() }\n}\n",
        )
        .write("specs/modules/Local.toml", &fw_module("Local", ""))
        .write(
            "src/local.rs",
            "mod inner { pub fn g() {} }\npub fn f() { self::inner::g(); core::mem::drop(0) }\n",
        );
    t.check(&[])
        .fires("internal_edges_declared", "ByValue", "error")
        .fires("internal_edges_declared", "ByType", "error")
        .fires("internal_edges_declared", "BySuper", "error")
        .silent("internal_edges_declared", "Declared")
        .silent("internal_edges_declared", "TestOnly")
        .silent("internal_edges_declared", "Local");
}

#[test]
fn report_imports_lists_a_use_path_once() {
    // A `use` line and the paths nested inside it are one import, not one per
    // path segment.
    let t = tree("imports-once", "firmware-layered", &[]);
    t.write("src/led.rs", "pub struct Led;\n").write(
        "src/menu.rs",
        "use crate::led::Led;\npub fn f(_l: &crate::led::Led) {}\n",
    );
    let found = imports_report(&t, "rust", "src");
    let menu: Vec<&str> = found
        .iter()
        .filter(|i| i.file == "src/menu.rs")
        .map(|i| i.spec.as_str())
        .collect();
    assert_eq!(
        menu,
        ["crate::led::Led", "crate::led::Led"],
        "one per use site"
    );
    resolves_to(&found, "src/menu.rs", "crate::led::Led", Some("src/led.rs"));
}

struct Import {
    file: String,
    spec: String,
    resolved: Option<String>,
}

fn imports_report(t: &Tree, language: &str, root: &str) -> Vec<Import> {
    let out = t.analyzer(&[
        "report",
        "imports",
        "--language",
        language,
        "--root",
        root,
        "--format",
        "yaml",
    ]);
    let v: serde_json::Value = serde_yaml::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "`report imports` did not run and report:\n{}",
            ci::text(&out)
        )
    });
    let mut found = Vec::new();
    for f in v["files"]
        .as_array()
        .unwrap_or_else(|| panic!("no `files` array:\n{}", ci::text(&out)))
    {
        for i in f["imports"].as_array().into_iter().flatten() {
            found.push(Import {
                file: f["file"].as_str().unwrap_or_default().to_string(),
                spec: i["spec"].as_str().unwrap_or_default().to_string(),
                resolved: i["resolved"].as_str().map(str::to_string),
            });
        }
    }
    found
}

#[track_caller]
fn resolves_to(found: &[Import], file: &str, spec: &str, target: Option<&str>) {
    let hit = found.iter().find(|i| i.file == file && i.spec == spec);
    let hit = hit.unwrap_or_else(|| panic!("no import `{spec}` in {file}"));
    assert_eq!(hit.resolved.as_deref(), target, "`{spec}` in {file}");
}

#[test]
fn c_and_cpp_quoted_includes_resolve_to_project_files() {
    let t = tree("includes", "library-api", &[]);
    t.write(
        "native/src/ring.c",
        "#include \"ring.h\"\n#include \"../inc/shared.h\"\n#include <stdio.h>\n",
    )
    .write("native/src/ring.h", "void ring(void);\n")
    .write("native/inc/shared.h", "int shared;\n")
    .write(
        "native/cpp/math.cpp",
        "#include \"math.hpp\"\n#include <vector>\n",
    )
    .write("native/cpp/math.hpp", "int add(int, int);\n");
    let c = imports_report(&t, "c", "native/src");
    resolves_to(&c, "native/src/ring.c", "ring.h", Some("native/src/ring.h"));
    resolves_to(
        &c,
        "native/src/ring.c",
        "../inc/shared.h",
        Some("native/inc/shared.h"),
    );
    assert!(
        !c.iter().any(|i| i.spec.contains("stdio")),
        "a system include is external"
    );
    let cpp = imports_report(&t, "cpp", "native/cpp");
    resolves_to(
        &cpp,
        "native/cpp/math.cpp",
        "math.hpp",
        Some("native/cpp/math.hpp"),
    );
}

#[test]
fn dart_package_imports_of_the_own_package_resolve_under_lib() {
    let t = tree("dart-imports", "library-api", &[]);
    t.write(
        "lib/app.dart",
        "import 'package:checked_intent_fixture/store.dart';\nimport 'widgets/card.dart';\nimport 'dart:core';\nimport 'package:other/thing.dart';\n",
    )
    .write("lib/store.dart", "class Store {}\n")
    .write("lib/widgets/card.dart", "class Card {}\n");
    let d = imports_report(&t, "dart", "lib");
    resolves_to(
        &d,
        "lib/app.dart",
        "package:checked_intent_fixture/store.dart",
        Some("lib/store.dart"),
    );
    resolves_to(
        &d,
        "lib/app.dart",
        "widgets/card.dart",
        Some("lib/widgets/card.dart"),
    );
    resolves_to(&d, "lib/app.dart", "package:other/thing.dart", None);
    resolves_to(&d, "lib/app.dart", "dart:core", None);
}

// ---- Open-question gate, workspace tier.

#[test]
fn open_question_gate_reads_the_workspace_tier() {
    let root = tree("oq-ws", "rust-workspace", &[]);
    let spec = root.read("spec.toml");
    // The first *uncommented* member table: the template also carries
    // commented-out examples of the same header.
    let start = spec
        .find("\n[[workspace.members]]")
        .map(|i| i + 1)
        .expect("template lists a member");
    root.write("spec.toml", &format!("{}[[workspace.members]]\nname = \"core\"\npath = \"crates/core\"\n", &spec[..start]))
        .write(
            "specs/open_questions/OQ-WS-001.toml",
            "[[open_question]]\nid = \"OQ-WS-001\"\ntitle = \"Workspace blocker\"\nstatus = \"open\"\ntext = \"Unknown.\"\n",
        );
    // A member library inside the workspace root.
    let member = tree("oq-ws-member", "library-api", &[]);
    let dest = root.root.join("crates/core");
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::rename(&member.root, &dest).unwrap();
    let m = Tree {
        root: dest,
        plugin: member.plugin,
    };
    m.write(
        "specs/modules/built.toml",
        "name = \"built\"\nvisibility = \"crate\"\ndescription = \"Built.\"\n",
    )
    .write("src/built.rs", "pub(crate) fn f() {}\n")
    .write(
        "specs/modules/later.toml",
        "name = \"later\"\nvisibility = \"crate\"\ndescription = \"Later.\"\nplanned = true\n",
    );
    m.check(&[])
        .fires("implementation_unblocked", "built", "error")
        .silent("implementation_unblocked", "later");
}

// ---- --since.

#[test]
fn since_reports_new_flags_flips_and_defaults() {
    let t = built("since", &["REQ-S-001"]);
    t.write("src/built_ok.rs", "pub fn built() {}\n")
        .write("specs/requirements/REQ-S-001.toml", &requirement("REQ-S-001", ""))
        .write(
            "specs/open_questions/OQ-S-001.toml",
            "[[open_question]]\nid = \"OQ-S-001\"\ntitle = \"Flip me\"\nstatus = \"open\"\ntext = \"?\"\naffects = [\"REQ-S-001\"]\n",
        )
        .write(
            "specs/open_questions/OQ-S-002.toml",
            "[[open_question]]\nid = \"OQ-S-002\"\ntitle = \"Soft\"\nstatus = \"open\"\nhard_gate = false\ntext = \"?\"\naffects = [\"REQ-S-001\"]\n",
        );
    t.commit_all("base");
    t.write("specs/modules/Later.toml", &fw_module("Later", "planned = true\n"))
        .write("specs/requirements/REQ-S-001.toml", &requirement("REQ-S-001", "test_pending = true\n"))
        .write(
            "specs/open_questions/OQ-S-001.toml",
            "[[open_question]]\nid = \"OQ-S-001\"\ntitle = \"Flip me\"\nstatus = \"open\"\nhard_gate = false\ntext = \"?\"\naffects = [\"REQ-S-001\"]\n",
        )
        .write(
            "specs/decisions/DEC-S-001.toml",
            "[[decision]]\nid = \"DEC-S-001\"\ntitle = \"Default for OQ-S-002\"\nstatus = \"accepted\"\ntext = \"Assume X.\"\ndefaults_for = [\"OQ-S-002\"]\n",
        )
        .write(
            "specs/open_questions/OQ-S-002.toml",
            "[[open_question]]\nid = \"OQ-S-002\"\ntitle = \"Soft\"\nstatus = \"default_committed\"\nhard_gate = false\ntext = \"?\"\ndefault_via = \"DEC-S-001\"\nverify = \"Check X.\"\naffects = [\"REQ-S-001\"]\n",
        );
    t.commit_all("change");
    t.check(&["--since", "HEAD~1"])
        .fires("since_new_flag", "Later", "info")
        .fires("since_new_flag", "REQ-S-001", "info")
        .fires("since_hard_gate_flip", "OQ-S-001", "info")
        .fires("since_default_committed", "OQ-S-002", "info")
        .silent("since_new_flag", "BuiltOk");
}

// ---- Ids in a document.

#[test]
fn ids_in_document_is_configured_in_domain_toml() {
    let t = built("ids-doc", &[]);
    t.write("src/built_ok.rs", "pub fn built() {}\n")
        .write("docs/PRD.md", "# PRD\n\nPer DEC-D-001 we parse in one pass.\n")
        .write("specs/decisions/DEC-D-001.toml", "[[decision]]\nid = \"DEC-D-001\"\ntitle = \"In the PRD\"\nstatus = \"accepted\"\ntext = \"One pass.\"\n")
        .write("specs/decisions/DEC-D-002.toml", "[[decision]]\nid = \"DEC-D-002\"\ntitle = \"Missing from the PRD\"\nstatus = \"accepted\"\ntext = \"Two pass.\"\n");
    // Severity lives in [validation.rules], like every graph check's.
    let domain = t.read("specs/specdev/domain.toml");
    assert!(domain.contains("[validation.rules]\n"), "{domain}");
    let domain = domain.replacen(
        "[validation.rules]\n",
        "[validation.rules]\nids_in_document = \"error\"\n",
        1,
    );
    t.write(
        "specs/specdev/domain.toml",
        &format!("{domain}\n[[project.graph_checks]]\nkind = \"ids_in_document\"\nrule = \"ids_in_document\"\nentity_scope = \"Decision\"\ndocument = \"docs/PRD.md\"\n"),
    );
    t.check(&[]).silent("ids_in_document", "DEC-D-001").fires(
        "ids_in_document",
        "DEC-D-002",
        "error",
    );
}
