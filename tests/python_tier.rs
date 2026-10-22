//! Python language tier: adapter resolution, item extraction over a
//! module and a `.pyi` stub, pytest discovery and node-id identities, import
//! resolution (relative, absolute, `from . import <submodule>`,
//! `if TYPE_CHECKING:`), the PEP 621 manifest, and the language enums.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use specdev_analyzer::import_edges::file_imports;
use specdev_analyzer::items::extract::extract_items_from_source;
use specdev_analyzer::lang::{adapter_for, ImportContext, LanguageAdapter};
use specdev_analyzer::test_discovery::discover_roots;

fn py() -> Box<dyn LanguageAdapter> {
    adapter_for("python").expect("python adapter resolves")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/python")
}

fn items(file: &str, ext: &str) -> BTreeSet<(String, String)> {
    let src = std::fs::read_to_string(fixture().join(file)).unwrap();
    extract_items_from_source(&src, ext, py().as_ref(), None)
        .expect("extract")
        .into_iter()
        .map(|i| (i.kind, i.name))
        .collect()
}

fn set(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
    pairs
        .iter()
        .map(|(k, n)| (k.to_string(), n.to_string()))
        .collect()
}

#[test]
fn adapter_surface() {
    let a = py();
    assert_eq!(a.name(), "python");
    assert_eq!(a.file_extensions(), vec!["py", "pyi"]);
    assert_eq!(a.path_separator(), ".");
}

#[test]
fn items_are_the_public_module_level_surface() {
    assert_eq!(
        items("src/midi_py/packet.py", "py"),
        set(&[
            ("const", "MAX_WORDS"),
            ("type", "Words"),
            ("type", "Group"),
            ("fn", "make"),
            ("fn", "fetch"),
            ("class", "Packet"),
            ("const", "Packet.MT"),
            ("fn", "Packet.message_type"),
            ("fn", "Packet.parse"),
            ("fn", "Packet.send"),
            ("class", "Kind"),
            ("const", "Kind.NOTE"),
        ]),
        "private names, dunders, members of `_Hidden`, nested and conditional \
         definitions are not items"
    );
}

#[test]
fn a_stub_file_gives_items_too() {
    assert_eq!(
        items("src/midi_py/_native.pyi", "pyi"),
        set(&[("fn", "parse"), ("class", "Native"), ("fn", "Native.words")])
    );
}

#[test]
fn pytest_discovery_and_node_id_identities() {
    let root = fixture();
    let disc = discover_roots(py().as_ref(), &root, &[root.join("tests")]).expect("discover");
    let ids: BTreeSet<String> = disc
        .tests
        .iter()
        .filter_map(|t| t.identity.clone())
        .collect();
    let want: BTreeSet<String> = [
        "test_roundtrip",
        "test_param",
        "test_async",
        "test_decorated",
        "test_inner",
        "test_unit",
        "test_skipped",
        "test_member_skipped",
        // The same method in two classes of one file: the pytest node id.
        "tests/test_packet.py::TestPacket::test_eq",
        "tests/test_packet.py::TestOther::test_eq",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        ids, want,
        "`Helper.test_ignored` and `Plain.test_not_collected` are not tests; conftest has none"
    );
    let pending: Vec<&str> = disc
        .tests
        .iter()
        .filter(|t| t.pending)
        .filter_map(|t| t.identity.as_deref())
        .collect();
    assert_eq!(pending, vec!["test_skipped", "test_member_skipped"]);
}

fn imports(file: &str, source_root: &str) -> Vec<(String, Option<String>, bool)> {
    let ctx = ImportContext {
        project_root: fixture(),
        source_root: PathBuf::from(source_root),
        package_name: "midi-py".into(),
        aliases: Vec::new(),
    };
    file_imports(py().as_ref(), &ctx, Path::new(file))
        .expect("imports")
        .into_iter()
        .map(|i| (i.spec, i.resolved, i.test_only))
        .collect()
}

#[test]
fn imports_resolve_relative_absolute_and_submodules() {
    let got = imports("src/midi_py/__init__.py", "src/midi_py");
    let resolved: BTreeSet<String> = got.iter().filter_map(|(_, r, _)| r.clone()).collect();
    for want in [
        "src/midi_py/packet.py",
        "src/midi_py/stream.py",
        "src/midi_py/_native.pyi",
        "src/midi_py/util.py",
    ] {
        assert!(resolved.contains(want), "missing {want}: {got:?}");
    }
    // `Packet` and `MAX_WORDS` are names in `.packet`, not submodules.
    assert!(
        !got.iter()
            .any(|(s, _, _)| s.ends_with("Packet") || s.ends_with("MAX_WORDS")),
        "{got:?}"
    );
}

#[test]
fn type_checking_imports_are_not_dependencies() {
    let got = imports("src/midi_py/packet.py", "src/midi_py");
    let stream = got
        .iter()
        .find(|(s, _, _)| s == ".stream")
        .unwrap_or_else(|| panic!("no .stream import: {got:?}"));
    assert!(
        stream.2,
        "the `if TYPE_CHECKING:` import is test_only: {got:?}"
    );
    let util = got.iter().find(|(s, _, _)| s == ".util").expect(".util");
    assert!(!util.2);
    // Third-party and standard-library modules resolve to nothing.
    let ext = got
        .iter()
        .find(|(s, _, _)| s == "dataclasses")
        .expect("dataclasses");
    assert!(ext.1.is_none());
}

#[test]
fn pyproject_is_the_manifest() {
    let dir = std::env::temp_dir().join(format!("specdev-pyproject-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"x\"\nversion = \"0.1.0\"\ndependencies = [\"attrs>=23\", \"Typing_Extensions\"]\n\
         [project.optional-dependencies]\nfast = [\"orjson>=3\"]\n\
         [dependency-groups]\ndev = [\"pytest>=8\"]\n",
    )
    .unwrap();
    let facts = py().manifest_facts(&dir).expect("facts");
    assert_eq!(facts.deps["attrs"][0].version.as_deref(), Some(">=23"));
    assert!(facts.deps.contains_key("typing-extensions"));
    assert!(facts.deps["orjson"][0].optional);
    assert!(facts.deps["pytest"][0].dev);
    assert_eq!(facts.package["version"], "0.1.0");
}

#[test]
fn the_language_enums_admit_python() {
    let embedded = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/embedded");
    for (file, ptr) in [
        (
            "spec.schema.yaml",
            "/properties/project/properties/language/enum",
        ),
        ("profile.schema.yaml", "/properties/language/enum"),
    ] {
        let v: serde_json::Value =
            serde_yaml::from_str(&std::fs::read_to_string(embedded.join(file)).unwrap()).unwrap();
        let admits = v
            .pointer(ptr)
            .and_then(|e| e.as_array())
            .is_some_and(|a| a.iter().any(|x| x == "python"));
        assert!(admits, "{file} does not admit python");
    }
}
