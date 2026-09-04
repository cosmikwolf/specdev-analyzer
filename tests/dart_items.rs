//! Dart items extraction + `items diff` end-to-end.
//!
//! Coverage:
//!   * The embedded Dart items.scm compiles and extracts every kind in
//!     the Dart tier vocabulary from a representative source file.
//!   * Lexical privacy: `_`-prefixed declarations (and members of
//!     `_`-prefixed hosts) are never items.
//!   * Members extract as `<Host>.<member>` with the Dart `.` separator,
//!     including ABSTRACT members (grammar wraps them in `declaration`,
//!     not `method_declaration`) and static methods.
//!   * Constructors and fields are not items (tier-documented rule).
//!   * CLI: `extract-items` and `items diff` reach exit 0 on a Dart
//!     project whose spec matches its source — the language seam works
//!     end-to-end without any Rust assumptions.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    items::extract::extract_items_from_source,
    lang::dart::DartAdapter,
};

fn full_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("specs")
        .join("full_plugin")
        .join("_plugin")
}

fn tmp_dir(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "specdev-dart-items-{tag}-{}-{}",
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

fn analyzer_bin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join("specdev-analyzer")
}

const DART_SOURCE: &str = r#"const int maxRetries = 3;

int add(int a, int b) => a + b;

int _hidden() => 0;

int get counter => 42;

set counter(int v) {}

typedef Predicate = bool Function(int value);

class Device {
  Device(this.id);
  final String id;
  Future<void> connect() async {}
  void _internal() {}
  static Device parse(String s) => Device(s);
  int get state => 0;
  set state(int v) {}
}

abstract class Scanner {
  void scan();
  int get count;
}

class _Private {
  void visible() {}
}

mixin Logging {
  void log(String m) {}
}

enum Status { idle, busy }

extension StatusX on Status {
  bool get isBusy => this == Status.busy;
}

extension type Meters(double v) {
  double get feet => v * 3.28;
}
"#;

#[test]
fn dart_extraction_covers_the_tier_vocabulary() {
    let items = extract_items_from_source(DART_SOURCE, &DartAdapter, None).unwrap();
    let pairs: Vec<(&str, &str)> = items
        .iter()
        .map(|i| (i.kind.as_str(), i.name.as_str()))
        .collect();

    for expected in [
        ("const", "maxRetries"),
        ("function", "add"),
        ("getter", "counter"),
        ("setter", "counter"),
        ("typedef", "Predicate"),
        ("class", "Device"),
        ("function", "Device.connect"),
        ("function", "Device.parse"),
        ("getter", "Device.state"),
        ("setter", "Device.state"),
        ("class", "Scanner"),
        ("function", "Scanner.scan"),
        ("getter", "Scanner.count"),
        ("mixin", "Logging"),
        ("function", "Logging.log"),
        ("enum", "Status"),
        ("extension", "StatusX"),
        ("getter", "StatusX.isBusy"),
        ("extension_type", "Meters"),
        ("getter", "Meters.feet"),
    ] {
        assert!(
            pairs.contains(&expected),
            "expected {expected:?} in extraction; got: {pairs:#?}"
        );
    }
}

#[test]
fn dart_privacy_and_non_item_shapes_are_filtered() {
    let items = extract_items_from_source(DART_SOURCE, &DartAdapter, None).unwrap();
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();

    // `_`-prefixed declarations and members of `_`-prefixed hosts.
    assert!(!names.contains(&"_hidden"), "got: {names:?}");
    assert!(!names.contains(&"Device._internal"), "got: {names:?}");
    assert!(!names.contains(&"_Private"), "got: {names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("_Private.")),
        "members of a private class are not reachable API; got: {names:?}"
    );

    // Constructors and fields are not items in v1 (tier-documented).
    assert!(
        !names.contains(&"Device.Device") && !names.contains(&"Device.new"),
        "constructors are not items; got: {names:?}"
    );
    assert!(!names.contains(&"Device.id"), "fields are not items; got: {names:?}");

    // Enum constants are not items — the enum declaration is one item.
    assert!(!names.contains(&"Status.idle"), "got: {names:?}");
}

/// `@override` members implement inherited contract (a widget's `build`,
/// `createState`, `toString`) — never new public surface. Concrete and
/// abstract members hang their annotations on different grammar nodes
/// (inside `method_declaration` vs on `class_member`), so both shapes
/// are exercised; a differently-annotated member must still extract.
#[test]
fn dart_override_members_are_not_items() {
    let src = r#"
class HomeScreen {
  @override
  Object build(Object context) => context;

  @override
  int get hashCode => 1;

  @Deprecated('old')
  void legacyHelper() {}

  void refresh() {}
}

abstract class Base {
  @override
  void poke();
}
"#;
    let items = extract_items_from_source(src, &DartAdapter, None).unwrap();
    let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();

    assert!(!names.contains(&"HomeScreen.build"), "got: {names:?}");
    assert!(!names.contains(&"HomeScreen.hashCode"), "got: {names:?}");
    assert!(!names.contains(&"Base.poke"), "abstract @override; got: {names:?}");
    assert!(
        names.contains(&"HomeScreen.legacyHelper"),
        "non-override annotation must not suppress; got: {names:?}"
    );
    assert!(names.contains(&"HomeScreen.refresh"), "got: {names:?}");
}

const DART_MODULE_SPEC: &str = r#"name = "BleScanner"
visibility = "pub"

[[items]]
name = "BleScanner"
kind = "class"
description = "Scans for BLE peripherals and exposes discovered devices."
satisfies = ["BLE-1"]

[[items]]
name = "BleScanner.start"
kind = "function"
description = "Begin scanning; emits devices on the stream."
satisfies = ["BLE-1"]

[[items]]
name = "BleScanner.isScanning"
kind = "getter"
description = "Whether a scan is in progress."
satisfies = ["BLE-1"]

[[items]]
name = "scanTimeout"
kind = "const"
description = "Default scan window."
satisfies = ["BLE-1"]

[[items]]
name = "DeviceFilter"
kind = "typedef"
description = "Predicate over discovered devices."
satisfies = ["BLE-1"]

[[items]]
name = "scanWindow"
kind = "getter"
description = "Current scan window."
satisfies = ["BLE-1"]

[[items]]
name = "scanWindow"
kind = "setter"
description = "Adjust the scan window."
satisfies = ["BLE-1"]

[[items]]
name = "scannerSmokeTest"
kind = "function"
description = "Unit test for the scanner surface."
test = true
test_kind = "unit"
verifies = ["BLE-1"]
"#;

const DART_MODULE_SOURCE: &str = r#"const Duration scanTimeout = Duration(seconds: 10);

Duration _window = Duration(seconds: 10);

// A getter/setter pair shares one name; item identity keys on
// (name, kind), so both spec entries must pair cleanly.
Duration get scanWindow => _window;

set scanWindow(Duration d) {
  _window = d;
}

typedef DeviceFilter = bool Function(String deviceId);

class BleScanner {
  bool _scanning = false;

  bool get isScanning => _scanning;

  void start(DeviceFilter filter) {
    _scanning = true;
  }
}
"#;

/// Build a Dart project the way a real one holds together at rest:
/// bootstrapped machinery, `spec.toml` declaring `language = "dart"`, and
/// the Dart language tier inlined under
/// `specs/specdev/schemas/inlined/languages/dart/`. (Bootstrap gates on
/// the profile shipping `codegen/<language>/`, which is profile content
/// this engine-side test doesn't carry — so bootstrap as rust, then flip
/// the language and inline the Dart tier, the same end state a
/// Dart-profile bootstrap produces.)
fn build_dart_project(tag: &str) -> PathBuf {
    let target = tmp_dir(tag);
    bootstrap::run_bootstrap(
        &full_plugin(),
        &BootstrapInputs {
            profile: "library-api".into(),
            language: "rust".into(),
            project_name: "blescan".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();

    let spec_path = target.join("spec.toml");
    let spec = std::fs::read_to_string(&spec_path).unwrap();
    assert!(spec.contains("language = \"rust\""), "fixture assumption");
    std::fs::write(&spec_path, spec.replace("language = \"rust\"", "language = \"dart\"")).unwrap();

    let dart_tier_dst = target.join("specs/specdev/schemas/inlined/languages/dart");
    std::fs::create_dir_all(&dart_tier_dst).unwrap();
    std::fs::copy(
        full_plugin().join("languages/dart/items.schema.yaml"),
        dart_tier_dst.join("items.schema.yaml"),
    )
    .unwrap();

    std::fs::write(target.join("specs/modules/BleScanner.toml"), DART_MODULE_SPEC).unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(target.join("src/ble_scanner.dart"), DART_MODULE_SOURCE).unwrap();
    target
}

#[test]
fn dart_extract_items_cli_exits_zero() {
    let target = build_dart_project("extract");
    let out = Command::new(analyzer_bin())
        .args(["extract-items", "--module", "BleScanner", "--project"])
        .arg(&target)
        .output()
        .expect("invoke extract-items");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "extract-items must exit 0 on a Dart module; stderr: {stderr}\nstdout: {stdout}"
    );
    assert!(stdout.contains("kind: class"), "stdout: {stdout}");
    assert!(stdout.contains("name: BleScanner.start"), "stdout: {stdout}");
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn dart_items_diff_cli_exits_zero_when_clean() {
    let target = build_dart_project("diff-clean");
    let out = Command::new(analyzer_bin())
        .args(["items", "diff", "--module", "BleScanner", "--project"])
        .arg(&target)
        .output()
        .expect("invoke items diff");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "items diff must exit 0 (clean) on a matching Dart module; \
         stderr: {stderr}\nstdout: {stdout}"
    );
    assert!(stdout.contains("in_spec_only: []"), "stdout: {stdout}");
    assert!(stdout.contains("in_source_only: []"), "stdout: {stdout}");
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn dart_items_diff_reports_drift_and_unknown_kind() {
    // Drift: spec names a member the source doesn't have → exit 1.
    let target = build_dart_project("diff-drift");
    let spec_path = target.join("specs/modules/BleScanner.toml");
    let mut spec = std::fs::read_to_string(&spec_path).unwrap();
    spec.push_str(
        "\n[[items]]\nname = \"BleScanner.stop\"\nkind = \"function\"\n\
         description = \"Stop scanning.\"\nsatisfies = [\"BLE-1\"]\n",
    );
    std::fs::write(&spec_path, &spec).unwrap();
    let out = Command::new(analyzer_bin())
        .args(["items", "diff", "--module", "BleScanner", "--project"])
        .arg(&target)
        .output()
        .expect("invoke items diff");
    assert_eq!(out.status.code(), Some(1), "missing member must be drift");

    // Unknown kind: a Rust kind is not in the Dart vocabulary → hard error
    // naming the Dart kind list (the vocabulary is per-language data).
    let spec = std::fs::read_to_string(&spec_path)
        .unwrap()
        .replace("kind = \"class\"", "kind = \"struct\"");
    std::fs::write(&spec_path, spec).unwrap();
    let out = Command::new(analyzer_bin())
        .args(["items", "diff", "--module", "BleScanner", "--project"])
        .arg(&target)
        .output()
        .expect("invoke items diff");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("ITEM-102"), "stderr: {stderr}");
    assert!(
        stderr.contains("class/mixin/enum/extension/extension_type/function/getter/setter/typedef/const"),
        "unknown-kind advice must list the Dart vocabulary; stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn dart_project_without_inlined_tier_errors_item_104() {
    // A Dart project whose language tier is NOT inlined must fail loudly
    // (no silent skip) as soon as a module declares items[].
    let target = build_dart_project("no-tier");
    std::fs::remove_dir_all(target.join("specs/specdev/schemas/inlined/languages/dart")).unwrap();
    let out = Command::new(analyzer_bin())
        .args(["items", "diff", "--module", "BleScanner", "--project"])
        .arg(&target)
        .output()
        .expect("invoke items diff");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("ITEM-104"), "stderr: {stderr}");
    assert!(stderr.contains("specdev migrate"), "stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&target);
}
