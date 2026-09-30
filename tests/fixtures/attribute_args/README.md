# Attribute-argument spike fixture — attribute values as data

`corpus/` is a verbatim copy of the `hello-rtic` dogfood tree:
`src/main.rs` and `specs/modules/{Ticker,Blinky}.toml`. `unit/` holds the two data files a rule
pack would ship: a tree-sitter query and a capture → spec-attribute mapping. The test harness in
`tests/attribute_argument_spike.rs` reads only these files; no unit name appears in its code.
