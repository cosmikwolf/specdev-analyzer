# nextjs-app profile

Spec-driven development for Next.js App Router applications on TypeScript, pnpm, Vitest and Playwright: modules
classified by architectural role, the route tree stated in the spec, and the client/server boundary carried as two
fields the compiler enforces.

## When to use this profile

Use `nextjs-app` when the project is a Next.js **application** on the
App Router — it ships to a browser, is never published to npm, and its
load-bearing concerns are the route tree and the client/server boundary.

Not this profile:

- A publishable TypeScript *library* — no profile covers that shape yet.
- A Pages Router application — out of scope.
- A monorepo with several packages — no profile drives a pnpm workspace root yet.

## Profile contents

| File | Purpose |
|------|---------|
| `profile.yaml` | Analyzer dispatch manifest (`language: typescript`, compile gates, `paired_files`) |
| `schemas/module.schema.yaml` | Module shape: `role`, `boundary_kind`, `app_path`, `client_component`, `environment`, deps, optional items — seven role-conditional rules |
| `schemas/architecture.schema.yaml` | Project decisions |
| `schemas/enums.schema.yaml` | Role / BoundaryKind / Environment / AppPath / identifier vocabularies |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | Ledger entry shapes |
| `reductions/project.yaml` | Entity/relation projection (Module with its five attrs, ledger kinds) |
| `domain.toml.template` | Graph checks incl. `role_dependency` over the nine roles |
| `guidance.md` | Implementation guidance, inlined to `specs/specdev/guidance/profile.md` |
| `templates/*.template` | Config files and the CI workflow, written once when absent |
| `spec.toml.template` | Bootstrap seed |

`package.json` is the project's: dependencies, scripts, `packageManager` and `engines` live there, not in the spec.

## Module shape

Every module carries a `role` — its architectural tier:

```
app > layout > page > route_handler > boundary > component > server_action > service > model
```

`app` owns the one root layout (`app/layout.tsx`); `model` is pure
types. The `role_dependency` graph check warns on upward dependencies.
Every `layout`, `page` and `boundary` module also gets a `renders` edge
from the `app` module (built from the roles, extends `contains`), so
`context --module <app module>` lists them under `contains`.
Same-role and downward dependencies are allowed.

Four more Next-shaped fields, each conditional on the role:

- **`app_path`** — required on `layout`, `page`, `route_handler`,
  `boundary`; forbidden elsewhere. The directory under `app/` where the
  module's route file lives (`/(marketing)/events/[id]`). Leading
  slash, no trailing slash, `/` is the root. Every Next marker is a
  legal segment; whitespace, `.`/`..`, `_`-prefixed and over-200-char
  segments are refused. The URL follows from it, never stored.
- **`boundary_kind`** — required on `boundary`: `loading`, `error`,
  `not-found`, `template`, `default`, `global-error`. The two error
  kinds require `client_component = true`; `global-error` requires
  `app_path = "/"`.
- **`client_component`** — allowed on `page`, `layout`, `boundary`,
  `component`. `true` means the body starts with `"use client"`.
- **`environment`** — required on `service`: `server`, `client` or
  `universal`. The body starts with `import "server-only"` /
  `import "client-only"` / nothing.

A minimal complete module file (`specs/modules/EventsPage.toml`):

```toml
name = "EventsPage"
role = "page"
app_path = "/events"
description = "Lists upcoming events with title, date and venue."
depends_on = ["EventRepository"]
satisfies = ["REQ-UI-001"]
```

and the service it depends on (`specs/modules/EventRepository.toml`):

```toml
name = "EventRepository"
role = "service"
environment = "server"
description = "Reads events from the database."
satisfies = ["REQ-DATA-001"]
```

## Route files

`app/events/page.tsx` is not a module. It is a two-line shim that re-exports its module body:

```ts
export * from "../../src/events_page";
export { default } from "../../src/events_page";
```

Three forms exist: this one (page, layout, app, most boundaries); the same with `"use client";` on top (`error`,
`global-error` — Next checks the directive in the route file itself); and a one-line `export *` for `route_handler`
(`route.ts` has no default export). Route segment config (`dynamic`, `revalidate`, `metadata`,
`generateStaticParams`, `generateMetadata`) lives in the body and is carried through `export *`; a named re-export is
a hard build error. Shims are written by hand, hold no logic, and use prettier's default shape.

## The client/server boundary

Next.js places a module by the import graph. The spec carries the two facts that decide it, and the body's first line
makes the compiler enforce them:

| spec | first line of the body | compiler effect |
|---|---|---|
| `client_component = true` | `"use client";` | everything it imports joins the client graph |
| `environment = "server"` | `import "server-only";` | an import from the client graph fails `next build` |
| `environment = "client"` | `import "client-only";` | an import from the server graph fails `next build` |
| `environment = "universal"` | *(none)* | documentation only |

There is **no validate-time client/server rule in v1**. Every wrong chain the design reviews built fails `next build`
through those imports; the fast gate (`next typegen`, `tsc --noEmit`) is blind to them, so a wrong chain surfaces at
the `next-build` gate. The enforcement holds only when the guard line is present — the guidance makes it
unconditional. Legal and silent: client component → `server_action` → server service (the form pattern); server page
→ client component → client service.

## Config files: written once, then yours

`specdev-analyzer bootstrap` writes, **only when absent**, `tsconfig.json`, `next.config.ts`, `vitest.config.ts`,
`playwright.config.ts`, `.gitignore`, `app/.gitkeep`, `pnpm-workspace.yaml` and `.github/workflows/specdev.yml` from
the profile templates. After that they are the project's. `next build` rewrites `tsconfig.json` when Next's keys are
missing; the template is authored in Next's own post-build shape (plus `strict: true`) so the file stays
byte-identical. `vitest.config.ts` aliases `server-only` to an empty module, because the package throws under
Vitest's default export condition.

`app/` must exist before the first gate: `next typegen` fails when it is absent and passes on an empty one, and
`tsc --noEmit` on a tree with no inputs fails until typegen has written `next-env.d.ts`. `app/.gitkeep` guarantees
it.

## Source layout

Each module lives in `src/<snake>.ts` or `.tsx` (`[paths] source = "src"`). Tests sit beside their module as
`src/<snake>.test.ts(x)`; e2e tests under `e2e/`.

## Requirements, decisions, open questions

One file per entry, under `specs/requirements/`, `specs/decisions/` and `specs/open_questions/`:

| Ledger | Role |
|---|---|
| requirements | the **what** — intent and constraints the code must satisfy; `verified_by` names the tests |
| decisions | the **how & why** — choices pinned over alternatives (`accepted` or `superseded`) |
| open questions | the **known unknowns** — `open`, `default_committed` or `closed` |

A decision reaches a module through the module's `guided_by[]`, the `decided_by[]` of a requirement it satisfies,
or the workspace tier; `specdev-analyzer report decisions --module <M>` lists them for whoever edits the code. An
`open` question that reaches a module with code fails `check` (`implementation_unblocked`); the exits are a decision
that closes it or a committed default with a `verify` line. Use `/spec-decide` and `/spec-question`.

## Graph checks shipped

- `unique_qualified_names` — every entity has a unique qualified name
- `modules_acyclic` — the module dependency graph has no cycles
- `role_dependency` — no upward dependency in the role ordering (warning)
- `single_app_module` — exactly one `app`-role module
- `requirement_ids_unique` — requirement ids are unique
- `decision_ids_unique` — decision ids are unique
- `requirements_have_satisfiers` — every requirement is satisfied by a module (error; a `planned` requirement is exempt)
- `decision_supersedes_acyclic` — supersession chains have no cycle
- `decision_supersedes_chain_consistency` — every superseded decision is named by exactly one accepted successor
- `open_question_closed_by_resolves` — `closed_by` names an existing decision
- `open_question_status_consistency` — `closed_by` / `default_via` name accepted decisions
- `implementation_unblocked` — the open-question gate, run by `check`: an `open` question reaches no module that has code

`check` adds the source rule, stale `planned` flags, imports against `depends_on`, and requirement ↔ test.

## Gates and CI

`specs/specdev/profile.yaml` declares the compile gates; the analyzer renders them into `scripts/specdev-gates.sh`,
which runs `specdev-analyzer check` first and then each gate. CI runs the script. Bootstrap writes
`.github/workflows/specdev.yml` once (yours afterwards); it reads the repository variable `SPECDEV_ANALYZER_REPO` and
the secret `SPECDEV_ANALYZER_TOKEN` to build the private analyzer.

**Toolchain CI installs:** Node (LTS) and pnpm (the workflow takes the version from `packageManager` in `package.json`), and the stable Rust toolchain to build `specdev-analyzer`.

CI installs are frozen: commit the lockfile, or the CI install refuses to change it.

| gate | steps |
|---|---|
| `fast` | `pnpm install` → `pnpm exec next typegen` → `pnpm exec tsc --noEmit` → `pnpm exec vitest run --passWithNoTests` |
| `next-build` | `pnpm exec next build` |

The script runs the gates in declared order, in one job: `next-build` has no install step of its own and relies on
`fast`'s `pnpm install`. Locally, `fast` may move the lockfile (no `--frozen-lockfile`); in CI, pnpm installs frozen
and refuses. `--passWithNoTests` hides a wrong `include` glob; the template pins it to `src/**`.

## Excluded in v1 (named, with triggers)

- **`global-not-found`, `forbidden`, `unauthorized`** — dead without an
  experimental `next.config.ts` flag (they build and never render).
  Trigger: the flag goes stable, or the driver project turns it on.
- **Metadata file conventions** (`sitemap`, `robots`, `manifest`,
  `icon`, `opengraph-image`, …). Trigger: the driver project needs one.
- **Multiple root layouts** (one per route group). Trigger: two shells
  with different `<html>`/`<body>`.
- **Validate-time URL uniqueness** and dynamic-segment name conflicts —
  `next build` fails on both. Trigger: hit more than once.
- **Validate-time client/server dependency warnings** — the compiler
  enforces the chain (above). Trigger: more than three `server-only` /
  `client-only` build errors where a validate warning would have saved
  a build.
- **A guard-line presence check** — the guard import is written by hand, following the guidance. Trigger: the first
  `service` body found without it.
- **Intercepting-route markers in the derived URL** — legal segments,
  not stripped in reports.

## Extending

Blocks contribute their own **top-level** namespaced tables to modules (composed through the module schema's `allOf`
aggregation); the `bindings` slot is reserved for *bridge* contributions. Inline blocks with `/spec-inline`. Each
block's guidance adds its framework rules on top of this profile's.
