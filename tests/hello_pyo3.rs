//! hello-pyo3: a Python package over a Rust extension, as two specdev
//! projects — `python-ffi` for the package, `library-api` with the `pyo3`
//! block for the crate. Both check clean; the native module's spec matches
//! its `.pyi` stub; the gate script runs the stub check on the
//! `[native] module` name; and a `[pyo3]` entry the code lacks is reported.
//! Building the extension needs maturin and a network, so the gates are run
//! by hand (phase receipts), not here.

use crate::ci;

const LIB_RS: &str = r#"use pyo3::exceptions::PyException;
use pyo3::prelude::*;

pyo3::create_exception!(_native, PacketError, PyException);

#[pyclass(frozen)]
pub struct Packet {
    words: Vec<u32>,
}

#[pyfunction]
fn parse(words: Vec<u32>) -> PyResult<Packet> {
    Ok(Packet { words })
}

#[pymodule]
mod _native {
    use pyo3::prelude::*;

    #[pymodule_export]
    use super::{parse, Packet, PacketError};
}
"#;

const BINDINGS: &str = r#"name = "bindings"
visibility = "pub"
description = "The Python surface."
source_file = "src/lib.rs"
satisfies = ["REQ-NAT-001"]

[pyo3]
modules = [{ name = "_native" }]
classes = [{ name = "Packet" }]
functions = [{ name = "parse" }]
exceptions = [{ name = "PacketError" }]
"#;

const STUB: &str = "from typing import final\n\n__all__ = [\"Packet\", \"PacketError\", \"parse\"]\n\n\
class PacketError(Exception): ...\n\n@final\nclass Packet:\n    @property\n    def words(self) -> list[int]: ...\n\n\
def parse(words: list[int]) -> Packet: ...\n";

const NATIVE: &str = r#"name = "_native"
description = "The compiled extension, described by its stub."
source_file = "src/checked_intent_fixture/_native.pyi"
satisfies = ["REQ-PKT-001"]

[[items]]
name = "Packet"
kind = "class"
description = "A packet of words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "Packet.words"
kind = "fn"
description = "The packet's words."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "PacketError"
kind = "class"
description = "Raised for a bad packet."
satisfies = ["REQ-PKT-001"]

[[items]]
name = "parse"
kind = "fn"
description = "Builds a packet."
satisfies = ["REQ-PKT-001"]
"#;

#[test]
fn hello_pyo3_two_packages_check_clean() {
    // The Rust crate: library-api with the pyo3 block.
    let crate_ = ci::tree_from(
        "hello-pyo3-rust",
        &ci::plugin_root(),
        "library-api",
        "rust",
        &["pyo3"],
    );
    crate_
        .write("src/lib.rs", LIB_RS)
        .write(
            "specs/requirements/REQ-NAT-001.toml",
            "[[requirement]]\nid = \"REQ-NAT-001\"\ntext = \"The crate shall expose packets to Python.\"\ntest_pending = true\n",
        )
        .write("specs/modules/bindings.toml", BINDINGS);
    let c = crate_.check(&[]);
    assert!(!c.ran().iter().any(|f| f.severity == "error"), "{}", c.raw);

    // A [pyo3] entry the code lacks is reported by the block's code check.
    crate_.write(
        "specs/modules/bindings.toml",
        &BINDINGS.replace(
            "{ name = \"parse\" }",
            "{ name = \"parse\" }, { name = \"version\" }",
        ),
    );
    crate_
        .check(&[])
        .fires("pack_entity_in_code", "bindings.version", "error");

    // The Python package: python-ffi.
    let pkg = ci::tree_lang("hello-pyo3-py", "python-ffi", "python");
    assert!(
        pkg.read("specs/architecture.toml")
            .contains("module = \"checked_intent_fixture._native\""),
        "the template seeds [native] module"
    );
    pkg.write(
        "src/checked_intent_fixture/__init__.py",
        "from ._native import Packet, PacketError, parse\n",
    )
    .write("src/checked_intent_fixture/_native.pyi", STUB)
    .write(
        "tests/test_packet.py",
        "from checked_intent_fixture import parse\n\ndef test_parse_builds_a_packet() -> None:\n    assert parse([1]).words == [1]\n",
    )
    .write("uv.lock", "version = 1\n")
    .write(
        "specs/requirements/REQ-PKT-001.toml",
        "[[requirement]]\nid = \"REQ-PKT-001\"\ntext = \"The package shall build packets.\"\nverified_by = [\"test_parse_builds_a_packet\"]\n",
    )
    .write("specs/modules/_native.toml", NATIVE);
    let c = pkg.check(&[]);
    assert!(!c.ran().iter().any(|f| f.severity == "error"), "{}", c.raw);

    let diff = pkg.analyzer(&["items", "diff", "--module", "_native"]);
    assert_eq!(diff.status.code(), Some(0), "{}", ci::text(&diff));

    let emit = pkg.analyzer(&["gate", "--emit"]);
    assert!(emit.status.success(), "{}", ci::text(&emit));
    let script = pkg.read("scripts/specdev-gates.sh");
    assert!(
        script.contains("'mypy.stubtest' 'checked_intent_fixture._native'"),
        "the stubs gate reads [native] module:\n{script}"
    );
}
