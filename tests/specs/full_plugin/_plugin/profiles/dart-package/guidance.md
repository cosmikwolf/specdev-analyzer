# dart-package — implementation guidance

Rules an agent follows when it writes or edits code in a `dart-package` package: a pure Dart package published to
pub.dev whose public surface is its contract. No check verifies these rules; `check` verifies the imports and the
tests, `items diff` the contract surface, the gates the analysis, the formatting and the tests.

## Layout and names

- The public entry library is `lib/<package>.dart`; it exports the libraries under `lib/src/` and holds no logic. A
  module is one library in `lib/src/`; `[paths].source` names that directory. A module named `PacketReader` is
  `lib/src/packet_reader.dart`; a library in a subdirectory sets `source_file`.
- Code outside the package imports only the entry library; `lib/src/` is the package's own. Inside the package,
  import a sibling with a relative path; tests import the package as users do (`package:<package>/<package>.dart`).
- A name that starts with `_` is library-private and never an item. Keep helpers private until a real consumer needs
  them.

## The contract surface

- When the module keeps `items[]`, each public class, mixin, enum, extension, function, getter, setter, typedef and
  top-level const is one item; a public member is `Class.member`. Describe each in prose — never by signature.
- An `@override` member implements an inherited contract and is not an item.
- Errors a caller should handle are classes that implement `Exception`, exported from the entry library; an `Error`
  is a programming mistake and is never caught to recover.
- Every public declaration has a `///` doc comment: pub.dev's score counts documentation, and the docs are the
  package's reference.

## Analysis, formatting and dependencies

- The analyze gate fails on any issue `analysis_options.yaml` enables, infos too; the format gate fails on code
  `dart format` would change. Both count in pub.dev's score. Fix the code — never weaken the analysis options to
  make it pass.
- `pubspec.yaml` is the truth for dependencies and the SDK constraint; the lint set and the test runner are dev
  dependencies. A regular package does not commit `pubspec.lock`, so the gates resolve the newest compatible
  versions each run.

## Tests

- Tests live in `test/`, in files named `*_test.dart`. A requirement's `verified_by` names a test by the identity
  `specdev-analyzer report tests` shows: its title, or its group path when titles repeat.
- An unfinished test is skipped with the requirement id in its skip reason; it never passes empty.
