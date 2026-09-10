//! TypeScript items extraction + `items diff` end-to-end.
//!
//! Coverage:
//!   * The embedded TypeScript items.scm compiles under BOTH grammars and
//!     extracts every kind in the tier vocabulary from representative
//!     `.ts` and `.tsx` source.
//!   * Visibility is `export`: non-exported top-level declarations, and
//!     every member of a non-exported class, are never items.
//!   * Member rule: `private` / `protected` / `#name` members are not
//!     items; absent modifier, `public`, `static`, `async`, generator
//!     members are. Constructors are not items. Accessor pairs extract as
//!     `getter` + `setter` under one name.
//!   * Overloads yield one item (the implementation), never three.
//!   * Re-exports: named specifiers (alias wins), `export * from` (named
//!     by the module specifier without quotes), `export * as ns from`.
//!   * `export` inside a `namespace` or `declare` block is not an item.
//!   * CLI: `extract-items` and `items diff` reach exit 0 on a TypeScript
//!     project with a `.ts` module and a `.tsx` module whose specs match
//!     their source — the language seam works end-to-end.

use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    items::extract::extract_items_from_source,
    lang::typescript::TypeScriptAdapter,
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
        "specdev-ts-items-{tag}-{}-{}",
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

const TS_SOURCE: &str = r#"export const maxRetries = 3;
export let mutableLimit = 10;
const hidden = 1;
export const [first, second] = [1, 2];

export function add(a: number, b: number): number {
  return a + b;
}

function helper(): void {}

export function over(a: string): void;
export function over(a: number): void;
export function over(a: string | number): void {
  void a;
}

export async function load(): Promise<void> {}
export function* gen(): Generator<number> {
  yield 1;
}

export interface Device {
  id: string;
  connect(): Promise<void>;
}

interface Internal {
  x: number;
}

export type Predicate = (v: number) => boolean;

export enum Status {
  Idle,
  Busy,
}

export class Scanner {
  #cache = 0;
  private secret = 1;
  public label = "scanner";
  constructor(readonly id: string) {}
  connect(): Promise<void> {
    return Promise.resolve();
  }
  private internal(): void {}
  protected guarded(): void {}
  public explicit(): void {}
  static parse(s: string): Scanner {
    return new Scanner(s);
  }
  get state(): number {
    return this.#cache;
  }
  set state(v: number) {
    this.#cache = v;
  }
  #hard(): void {}
  async *stream(): AsyncGenerator<number> {
    yield 1;
  }
  getValue(): number {
    return 1;
  }
}

export abstract class Base {
  abstract scan(): void;
  abstract get count(): number;
  protected abstract hiddenHook(): void;
  concrete(): void {}
}

class Private {
  visible(): void {}
}

export default class Store {
  put(key: string): void {
    void key;
  }
}

export { helper as exposedHelper, hidden };
export * from "./barrel";
export * as util from "./util";
export type { Internal as Exposed };

export namespace NS {
  export const inner = 1;
}

declare module "node:fs" {
  export interface Dirent {
    extra?: string;
  }
}
"#;

const TSX_SOURCE: &str = r#"import type { ReactNode } from "react";

export interface CardProps {
  title: string;
  children?: ReactNode;
}

export function EventCard({ title, children }: CardProps): ReactNode {
  return <section><h2>{title}</h2>{children}</section>;
}

export const Badge = ({ label }: { label: string }) => <span>{label}</span>;

function LocalOnly(): ReactNode {
  return <i />;
}

export class Counter {
  #count = 0;
  private step = 1;
  next(): number {
    this.#count += this.step;
    return this.#count;
  }
  render(): ReactNode {
    return <em>{this.next()}</em>;
  }
}

export default function Page(): ReactNode {
  return <EventCard title="x"><Badge label="new" /></EventCard>;
}
"#;

fn pairs(src: &str, ext: &str) -> Vec<(String, String)> {
    extract_items_from_source(src, ext, &TypeScriptAdapter, None)
        .unwrap()
        .into_iter()
        .map(|i| (i.kind, i.name))
        .collect()
}

#[test]
fn typescript_extraction_covers_the_tier_vocabulary() {
    let got = pairs(TS_SOURCE, "ts");
    for (kind, name) in [
        ("const", "maxRetries"),
        ("const", "mutableLimit"),
        ("function", "add"),
        ("function", "over"),
        ("function", "load"),
        ("function", "gen"),
        ("interface", "Device"),
        ("type", "Predicate"),
        ("enum", "Status"),
        ("class", "Scanner"),
        ("function", "Scanner.connect"),
        ("function", "Scanner.explicit"),
        ("function", "Scanner.parse"),
        ("function", "Scanner.stream"),
        ("function", "Scanner.getValue"),
        ("getter", "Scanner.state"),
        ("setter", "Scanner.state"),
        ("class", "Base"),
        ("function", "Base.scan"),
        ("getter", "Base.count"),
        ("function", "Base.concrete"),
        ("class", "Store"),
        ("function", "Store.put"),
        ("re_export", "exposedHelper"),
        ("re_export", "hidden"),
        ("re_export", "./barrel"),
        ("re_export", "util"),
        ("re_export", "Exposed"),
    ] {
        assert!(
            got.contains(&(kind.to_string(), name.to_string())),
            "expected ({kind}, {name}) in extraction; got: {got:#?}"
        );
    }
    // Overloads: exactly one `function over`.
    let overs = got
        .iter()
        .filter(|(k, n)| k == "function" && n == "over")
        .count();
    assert_eq!(
        overs, 1,
        "overload signatures collapse into the implementation; got: {got:#?}"
    );
}

#[test]
fn typescript_visibility_and_non_item_shapes_are_filtered() {
    let got = pairs(TS_SOURCE, "ts");
    let names: Vec<&str> = got.iter().map(|(_, n)| n.as_str()).collect();

    // Not exported → not an item (top level).
    for n in ["helper", "Internal", "Private"] {
        assert!(!names.contains(&n), "`{n}` is not exported; got: {names:?}");
    }
    // Members of a non-exported class are unreachable.
    assert!(
        !names.iter().any(|n| n.starts_with("Private.")),
        "got: {names:?}"
    );
    // `hidden` appears ONLY as a re_export (the `export { hidden }`), never as a const.
    assert!(
        !got.contains(&("const".to_string(), "hidden".to_string())),
        "got: {got:#?}"
    );
    // Destructured bindings have no identifier name.
    assert!(
        !names.contains(&"first") && !names.contains(&"second"),
        "got: {names:?}"
    );

    // Member rule.
    for n in [
        "Scanner.internal",
        "Scanner.guarded",
        "Scanner.#cache",
        "Scanner.#hard",
        "Scanner.secret",
        "Scanner.label",
        "Scanner.constructor",
        "Base.hiddenHook",
    ] {
        assert!(
            !names.contains(&n),
            "`{n}` must not be an item; got: {names:?}"
        );
    }
    // Accessors are not also methods.
    assert!(
        !got.contains(&("function".to_string(), "Scanner.state".to_string())),
        "an accessor is a getter/setter, never a function; got: {got:#?}"
    );
    // Interface members, enum constants, namespace and declare bodies.
    for n in [
        "Device.connect",
        "Device.id",
        "Status.Idle",
        "NS.inner",
        "inner",
        "Dirent",
    ] {
        assert!(
            !names.contains(&n),
            "`{n}` must not be an item; got: {names:?}"
        );
    }
    // The namespace itself is not a v1 kind either.
    assert!(!names.contains(&"NS"), "got: {names:?}");
}

#[test]
fn typescript_extraction_works_under_the_tsx_grammar() {
    let got = pairs(TSX_SOURCE, "tsx");
    for (kind, name) in [
        ("interface", "CardProps"),
        ("function", "EventCard"),
        ("const", "Badge"),
        ("class", "Counter"),
        ("function", "Counter.next"),
        ("function", "Counter.render"),
        ("function", "Page"),
    ] {
        assert!(
            got.contains(&(kind.to_string(), name.to_string())),
            "expected ({kind}, {name}); got: {got:#?}"
        );
    }
    let names: Vec<&str> = got.iter().map(|(_, n)| n.as_str()).collect();
    assert!(!names.contains(&"LocalOnly"), "got: {names:?}");
    assert!(!names.contains(&"Counter.step"), "got: {names:?}");
    assert!(!names.contains(&"Counter.#count"), "got: {names:?}");
    // And the .tsx source under the .ts grammar is a parse failure, not a
    // silently-empty extraction — the extractor must not hide it.
    // (tree-sitter still returns a tree with ERROR nodes; items may be
    // partial. The adapter test asserts has_error; here we only assert the
    // right grammar yields the JSX component.)
}

const TS_MODULE_SPEC: &str = r#"name = "BleScanner"
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
description = "Default scan window in milliseconds."
satisfies = ["BLE-1"]

[[items]]
name = "DeviceFilter"
kind = "type"
description = "Predicate over discovered devices."
satisfies = ["BLE-1"]

[[items]]
name = "ScanOptions"
kind = "interface"
description = "Options accepted by start()."
satisfies = ["BLE-1"]

[[items]]
name = "scannerSmokeTest"
kind = "function"
description = "Unit test for the scanner surface."
test = true
test_kind = "unit"
verifies = ["BLE-1"]
"#;

const TS_MODULE_SOURCE: &str = r#"export const scanTimeout = 10_000;

export type DeviceFilter = (deviceId: string) => boolean;

export interface ScanOptions {
  filter?: DeviceFilter;
}

export class BleScanner {
  #scanning = false;

  get isScanning(): boolean {
    return this.#scanning;
  }

  start(options: ScanOptions = {}): void {
    void options;
    this.#scanning = true;
  }
}
"#;

const TSX_MODULE_SPEC: &str = r#"name = "EventCard"
visibility = "pub"

[[items]]
name = "EventCardProps"
kind = "interface"
description = "Props for the event card."
satisfies = ["UI-1"]

[[items]]
name = "EventCard"
kind = "function"
description = "Renders one event as a card."
satisfies = ["UI-1"]
"#;

const TSX_MODULE_SOURCE: &str = r#"import type { ReactNode } from "react";

export interface EventCardProps {
  title: string;
}

export function EventCard({ title }: EventCardProps): ReactNode {
  return <article>{title}</article>;
}
"#;

/// Build a TypeScript project the way a real one holds together at rest:
/// bootstrapped machinery, `spec.toml` declaring `language = "typescript"`,
/// and the TypeScript language tier inlined under
/// `specs/specdev/schemas/inlined/languages/typescript/`. (Bootstrap gates
/// on the profile shipping `codegen/<language>/`, which is profile content
/// this engine-side test doesn't carry — so bootstrap as rust, then flip
/// the language and inline the tier, the same end state a TypeScript-
/// profile bootstrap produces.)
fn build_ts_project(tag: &str) -> PathBuf {
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
    std::fs::write(
        &spec_path,
        spec.replace("language = \"rust\"", "language = \"typescript\""),
    )
    .unwrap();

    let tier_dst = target.join("specs/specdev/schemas/inlined/languages/typescript");
    std::fs::create_dir_all(&tier_dst).unwrap();
    std::fs::copy(
        full_plugin().join("languages/typescript/items.schema.yaml"),
        tier_dst.join("items.schema.yaml"),
    )
    .unwrap();

    std::fs::write(target.join("specs/modules/BleScanner.toml"), TS_MODULE_SPEC).unwrap();
    std::fs::write(target.join("specs/modules/EventCard.toml"), TSX_MODULE_SPEC).unwrap();
    std::fs::create_dir_all(target.join("src")).unwrap();
    std::fs::write(target.join("src/ble_scanner.ts"), TS_MODULE_SOURCE).unwrap();
    std::fs::write(target.join("src/event_card.tsx"), TSX_MODULE_SOURCE).unwrap();
    target
}

fn run(target: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(analyzer_bin())
        .args(args)
        .arg("--project")
        .arg(target)
        .output()
        .expect("invoke analyzer");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn typescript_extract_items_cli_exits_zero_for_ts_and_tsx_modules() {
    let target = build_ts_project("extract");
    let (code, stdout, stderr) = run(&target, &["extract-items", "--module", "BleScanner"]);
    assert_eq!(code, Some(0), "stderr: {stderr}\nstdout: {stdout}");
    assert!(stdout.contains("kind: class"), "stdout: {stdout}");
    assert!(
        stdout.contains("name: BleScanner.start"),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("name: BleScanner.isScanning"),
        "stdout: {stdout}"
    );

    // The .tsx module resolves through the adapter's candidate list and
    // parses under the tsx grammar — JSX in the body is not an error.
    let (code, stdout, stderr) = run(&target, &["extract-items", "--module", "EventCard"]);
    assert_eq!(code, Some(0), "stderr: {stderr}\nstdout: {stdout}");
    assert!(stdout.contains("name: EventCard"), "stdout: {stdout}");
    assert!(stdout.contains("name: EventCardProps"), "stdout: {stdout}");
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn typescript_items_diff_cli_exits_zero_when_clean() {
    let target = build_ts_project("diff-clean");
    for module in ["BleScanner", "EventCard"] {
        let (code, stdout, stderr) = run(&target, &["items", "diff", "--module", module]);
        assert_eq!(
            code,
            Some(0),
            "items diff must exit 0 (clean) on `{module}`; stderr: {stderr}\nstdout: {stdout}"
        );
        assert!(stdout.contains("in_spec_only: []"), "{module}: {stdout}");
        assert!(stdout.contains("in_source_only: []"), "{module}: {stdout}");
    }
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn typescript_items_diff_reports_drift_and_unknown_kind() {
    let target = build_ts_project("diff-drift");
    let spec_path = target.join("specs/modules/BleScanner.toml");
    let mut spec = std::fs::read_to_string(&spec_path).unwrap();
    spec.push_str(
        "\n[[items]]\nname = \"BleScanner.stop\"\nkind = \"function\"\n\
         description = \"Stop scanning.\"\nsatisfies = [\"BLE-1\"]\n",
    );
    std::fs::write(&spec_path, &spec).unwrap();
    let (code, _, _) = run(&target, &["items", "diff", "--module", "BleScanner"]);
    assert_eq!(code, Some(1), "missing member must be drift");

    // A Rust kind is not in the TypeScript vocabulary → hard error naming
    // the TypeScript kind list (the vocabulary is per-language data).
    let spec = std::fs::read_to_string(&spec_path)
        .unwrap()
        .replace("kind = \"class\"", "kind = \"struct\"");
    std::fs::write(&spec_path, spec).unwrap();
    let (code, _, stderr) = run(&target, &["items", "diff", "--module", "BleScanner"]);
    assert_ne!(code, Some(0));
    assert!(stderr.contains("ITEM-102"), "stderr: {stderr}");
    assert!(
        stderr.contains("function/class/interface/type/enum/const/getter/setter/re_export"),
        "unknown-kind advice must list the TypeScript vocabulary; stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&target);
}
