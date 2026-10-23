//! hello-ts-library and hello-dart-package: each library profile through the
//! pipeline — bootstrap, a module with items and tests, `check` with zero
//! errors, a clean `items diff`, and the gates in the rendered gate script.
//! The gates themselves need Node or the Dart SDK, so they are run by hand
//! (phase receipts), not here.

use crate::ci;

fn assert_clean(t: &ci::Tree, module: &str) {
    let c = t.check(&[]);
    assert!(!c.ran().iter().any(|f| f.severity == "error"), "{}", c.raw);
    let diff = t.analyzer(&["items", "diff", "--module", module]);
    assert_eq!(diff.status.code(), Some(0), "{}", ci::text(&diff));
}

const ITEMS: &str = r#"
[[items]]
name = "Packet"
kind = "class"
description = "A packet of one to four words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "parse"
kind = "function"
description = "Builds a packet from its words."
satisfies = ["REQ-PKT-001"]
"#;

const REQ: &str =
    "[[requirement]]\nid = \"REQ-PKT-001\"\ntext = \"The package shall build packets.\"\n\
verified_by = [\"builds a packet\"]\n";

#[test]
fn hello_ts_library() {
    let t = ci::tree_lang("hello-ts-library", "ts-library", "typescript");
    for f in ["tsconfig.json", "tsconfig.build.json", ".gitignore"] {
        assert!(t.root.join(f).is_file(), "paired file {f} written");
    }
    t.write(
        "src/packet.ts",
        "export class Packet {\n  constructor(readonly words: readonly number[]) {}\n}\n\n\
         export function parse(words: readonly number[]): Packet {\n  return new Packet(words);\n}\n",
    )
    .write(
        "src/packet.test.ts",
        "import { expect, test } from \"vitest\";\nimport { parse } from \"./packet.js\";\n\n\
         test(\"builds a packet\", () => {\n  expect(parse([1]).words).toEqual([1]);\n});\n",
    )
    .write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n")
    .write("specs/requirements/REQ-PKT-001.toml", REQ)
    .write(
        "specs/modules/packet.toml",
        &format!("name = \"packet\"\ndescription = \"Packets.\"\nsatisfies = [\"REQ-PKT-001\"]\n{ITEMS}"),
    );
    assert_clean(&t, "packet");
    let emit = t.analyzer(&["gate", "--emit"]);
    assert!(emit.status.success(), "{}", ci::text(&emit));
    let script = t.read("scripts/specdev-gates.sh");
    for want in [
        "'pnpm' 'install' '--frozen-lockfile'",
        "'tsc' '--noEmit'",
        "'vitest' 'run'",
        "'tsconfig.build.json'",
    ] {
        assert!(script.contains(want), "missing {want}:\n{script}");
    }
}

#[test]
fn hello_dart_package() {
    let t = ci::tree_lang("hello-dart-package", "dart-package", "dart");
    assert!(t.root.join("analysis_options.yaml").is_file());
    t.write(
        "lib/src/packet.dart",
        "/// A packet.\nclass Packet {\n  /// Builds it.\n  Packet(this.words);\n\n  /// Its words.\n  final List<int> words;\n}\n\n\
         /// Builds a packet.\nPacket parse(List<int> words) => Packet(words);\n",
    )
    .write("lib/checked_intent_fixture.dart", "export 'src/packet.dart';\n")
    .write(
        "test/packet_test.dart",
        "import 'package:checked_intent_fixture/checked_intent_fixture.dart';\nimport 'package:test/test.dart';\n\n\
         void main() {\n  test('builds a packet', () {\n    expect(parse([1]).words, [1]);\n  });\n}\n",
    )
    .write("specs/requirements/REQ-PKT-001.toml", REQ)
    .write(
        "specs/modules/packet.toml",
        &format!("name = \"packet\"\ndescription = \"Packets.\"\nsatisfies = [\"REQ-PKT-001\"]\n{ITEMS}"),
    );
    assert_clean(&t, "packet");
    let emit = t.analyzer(&["gate", "--emit"]);
    assert!(emit.status.success(), "{}", ci::text(&emit));
    let script = t.read("scripts/specdev-gates.sh");
    for want in [
        "'dart' 'pub' 'get'",
        "'--fatal-infos'",
        "'--set-exit-if-changed'",
        "'dart' 'test'",
    ] {
        assert!(script.contains(want), "missing {want}:\n{script}");
    }
}
