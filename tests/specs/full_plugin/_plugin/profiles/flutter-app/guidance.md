# flutter-app — implementation guidance

Rules an agent follows when it writes or edits code in a `flutter-app` package. No check verifies these rules;
`check` verifies the roles, the imports and the tests, and review the rest.

## Files

- One module, one file: `lib/src/<snake_name>.dart`; private helpers stay in it with `_` names.
- `lib/main.dart` only delegates to the `app` module, which holds the app shell, routes, theme and dependency setup.
  No barrel exports in an app.
- Imports inside `lib/` are package imports. Add the `depends_on` edge before importing a sibling.

## Roles

- `app` → `screen` → `widget` → `service` → `platform_channel` → `model`; dependencies point down or stay in the
  role. Nothing depends on `app`: screens navigate by route name, never by importing the route table.
- A screen is one widget named after its module, stateless with state in a service; local state only for things
  like controllers.
- Reusable widgets take data and callbacks as parameters.
- A service exposes state as notifiers or streams, and its providers live with it, not in `app`.
- UI reaches platform channels through a service.
- A model imports no Flutter: immutable, `const` constructors, `copyWith`, sealed hierarchies for closed variants.

## Platforms

- A module without `platforms` supports the whole matrix in `architecture.toml`; one with `platforms` is fenced to
  that subset. Each file's doc header states its platform contract.
- Code that runs on web never imports a module fenced away from web without a guard: a runtime web check when both
  branches compile, a conditional export when one does not.

## Platform channels

- Channel names come from the spec verbatim, one constant each. `method` channels convert platform exceptions into
  the module's typed failures; `event` channels are typed streams; `basic_message` needs an explicit codec.
- Native handlers live in platform directories the user owns; the module's doc header lists what they must
  implement.

## Dart and Flutter

- No `dynamic` in public surface; `final` by default; `late` only when order requires it; `const` wherever allowed.
- Throw typed exceptions per failure family, never strings. No fire-and-forget futures without a stated reason.
- Check widget constructors against the installed Flutter SDK, not memory: widget APIs are deprecated and replaced.
- A method that ends an in-flight operation guards on the in-flight flag alone, then branches on the error. A guard
  that also tests the error lets a normal completion fall into the failure path.
- Never ship an `UnimplementedError` body. An unfinished test fails loudly and names its requirement.

## Generated and owned files

- Platform directories and SDK files belong to Flutter tooling; `*.g.dart` and bridge glue belong to their
  generators. Never hand-edit them.
- Delete the scaffold's template widget test after `flutter create`.
