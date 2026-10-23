# ts-library — implementation guidance

Rules an agent follows when it writes or edits code in a `ts-library` package: a TypeScript package published to npm
whose exported surface is its contract. No check verifies these rules; `check` verifies the imports and the tests,
`items diff` the contract surface, the gates the types, the tests and the build.

## Layout and names

- Sources are in `src/`; the package entry is `src/index.ts`, which re-exports the public names and holds no logic.
  A module named `PacketStore` is `src/packet_store.ts` (or `src/packet_store/index.ts`); a file named another way
  sets `source_file` in its spec.
- The package is ESM (`"type": "module"`). Relative imports name the emitted file with its `.js` extension
  (`./packet.js` for `packet.ts`): the module resolution the tsconfig sets requires it, and `check` resolves it.
- `package.json` `exports` names the entry: its `types` condition first, pointing at the emitted declarations in
  `dist/`, then the JavaScript. Only `dist/` is published (`files`).
- An import used only as a type is `import type`; the tsconfig refuses the plain form for it.

## The contract surface

- When the module keeps `items[]`, each exported function, class, interface, type alias, enum and const is one
  item; a public class member is `Class.member`. Describe each in prose — never by signature or generic parameters.
- Anything not exported from the module is private and is never an item. Do not export a helper for a test: test it
  through the public API.
- Errors: the package defines its error classes (extending `Error`) and exports them; public code throws them,
  never a bare string or a plain object.
- Every exported function states its return type, so the emitted declarations do not change when the body does.

## Dependencies and the tool

- `package.json` is the truth for dependencies and the `exports` map; the TypeScript compiler and the test runner
  are dev dependencies. Declare the package manager in `packageManager`.
- The tool `spec.toml` declares writes the lockfile; commit it, and leave no lockfile of another tool. The gates
  install from it frozen.
- `dist/` is build output: never committed, never edited, never a spec module.

## Tests

- A test file sits beside its module (`packet.test.ts`); the build config leaves test files out of `dist/`.
- A requirement's `verified_by` names a test by the identity `specdev-analyzer report tests` shows: its title, or
  its group path when titles repeat.
- An unfinished test is a skipped test whose title names the requirement it owes; it never passes empty.
