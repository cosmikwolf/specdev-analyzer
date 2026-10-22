//! hello-python: the `python-library` profile through the pipeline — bootstrap,
//! a module with items and pytest tests, `check` with zero errors, a clean
//! `items diff`, and the declared tool's gates in the rendered gate script.
//! The gates themselves need uv, so they are run by hand (phase receipts),
//! not here.

use crate::ci;

const PACKET: &str = r#""""One packet."""
from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Sequence

MAX_WORDS = 4


class PacketError(Exception):
    """A packet could not be built."""


class Packet:
    """A packet of 32-bit words."""

    def __init__(self, words: list[int]) -> None:
        self.words = words

    @staticmethod
    def parse(words: Sequence[int]) -> Packet:
        if not 1 <= len(words) <= MAX_WORDS:
            raise PacketError(f"bad length {len(words)}")
        return Packet(list(words))
"#;

const TESTS: &str = "from checked_intent_fixture.packet import Packet\n\n\
def test_parse_accepts_one_to_four_words() -> None:\n    assert Packet.parse([1]).words == [1]\n\n\
class TestErrors:\n    def test_rejects_five_words(self) -> None:\n        pass\n";

const MODULE: &str = r#"name = "packet"
description = "Builds packets from words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "Packet"
kind = "class"
description = "A packet of 32-bit words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "Packet.parse"
kind = "fn"
description = "Builds a packet; rejects a length outside one to four words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "PacketError"
kind = "class"
description = "Raised when a packet cannot be built."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "MAX_WORDS"
kind = "const"
description = "The longest packet, in words."
satisfies = ["REQ-PKT-001"]
"#;

#[test]
fn hello_python_checks_clean_and_renders_the_uv_gates() {
    let t = ci::tree_lang("hello-python", "python-library", "python");
    assert!(
        t.read("spec.toml")
            .contains("source = \"src/checked_intent_fixture\""),
        "the template points [paths].source at the import package"
    );
    t.write("src/checked_intent_fixture/__init__.py", "from .packet import Packet\n")
        .write("src/checked_intent_fixture/packet.py", PACKET)
        .write("tests/test_packet.py", TESTS)
        .write("uv.lock", "version = 1\n")
        .write(
            "specs/requirements/REQ-PKT-001.toml",
            "[[requirement]]\nid = \"REQ-PKT-001\"\ntext = \"The package shall build a packet from one to four words.\"\n\
             verified_by = [\"test_parse_accepts_one_to_four_words\", \"test_rejects_five_words\"]\n",
        )
        .write("specs/modules/packet.toml", MODULE);
    let c = t.check(&[]);
    let errors: Vec<_> = c.ran().iter().filter(|f| f.severity == "error").collect();
    assert!(errors.is_empty(), "{}", c.raw);
    assert!(
        !c.ran().iter().any(|f| f.rule.starts_with("toolchain_")),
        "uv declared and uv.lock present: no toolchain finding\n{}",
        c.raw
    );

    let diff = t.analyzer(&["items", "diff", "--module", "packet"]);
    assert_eq!(diff.status.code(), Some(0), "{}", ci::text(&diff));

    let emit = t.analyzer(&["gate", "--emit"]);
    assert!(emit.status.success(), "{}", ci::text(&emit));
    let script = t.read("scripts/specdev-gates.sh");
    for want in [
        "'uv' 'sync' '--locked'",
        "'pytest'",
        "'pyright'",
        "'ruff' 'check'",
    ] {
        assert!(script.contains(want), "missing {want}:\n{script}");
    }
    assert!(!script.contains("'poetry'"), "{script}");
}
