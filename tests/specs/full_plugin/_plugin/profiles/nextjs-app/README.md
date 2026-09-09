# nextjs-app profile

Spec-driven development for Next.js 16 App Router applications on
TypeScript, pnpm, Vitest and Playwright: modules classified by
architectural role, route files derived from the spec, and the
client/server boundary carried as two fields the compiler enforces.

**Status:** this profile bootstraps, validates and emits `package.json`
plus its config files. Route-file derivation (`specdev-analyzer derive`)
and the gate runner (`specdev-analyzer gate`) are not yet shipped; until
they are, `/spec-gen` cannot complete a nextjs-app tree.

## When to use this profile

Use `nextjs-app` when the project is a Next.js **application** on the
App Router — it ships to a browser, is never published to npm, and its
load-bearing concerns are the route tree, the client/server boundary,
and its npm dependencies.

Not this profile:

- A publishable TypeScript *library* — no profile covers that shape yet.
- A Pages Router application — out of scope.
- A monorepo with several packages — the pnpm workspace root shape
  exists in the manifest emitter, but no profile drives it yet.

## Profile contents

| File | Purpose |
|------|---------|
| `profile.yaml` | Analyzer dispatch manifest (`language: typescript`, package.json shape `app`, pins, scripts, `paired_files`) |
| `schemas/module.schema.yaml` | Module shape: `role`, `boundary_kind`, `app_path`, `client_component`, `environment`, deps, items — seven role-conditional rules |
| `schemas/architecture.schema.yaml` | App version, Node / package-manager pins, script overrides, project-wide deps |
| `schemas/enums.schema.yaml` | Role / BoundaryKind / Environment / AppPath / identifier vocabularies |
| `schemas/requirements.schema.yaml`, `decisions.schema.yaml`, `open_questions.schema.yaml` | The triad (same shapes as the other profiles) |
| `reductions/project.yaml` | Entity/relation projection (Module with its five attrs, triad kinds) |
| `domain.toml.template` | Graph checks incl. `role_dependency` over the nine roles |
| `templates/*.template` | Six config files written once when absent; three route-shim forms |
| `codegen/typescript/module.conventions.md` | How a module spec maps to TypeScript source, and the client/server rules |
| `*.toml.template` | Bootstrap seeds for spec.toml / architecture / requirements / decisions / open questions |

## Module shape

Every module carries a `role` — its architectural tier:

```
app > layout > page > route_handler > boundary > component > server_action > service > model
```

`app` owns the one root layout (`app/layout.tsx`); `model` is pure
types. The `role_dependency` graph check warns on upward dependencies.
Same-role and downward dependencies are allowed.

Four more Next-shaped fields, each conditional on the role:

- **`app_path`** — required on `layout`, `page`, `route_handler`,
  `boundary`; forbidden elsewhere. The directory under `app/` where the
  module's route file is derived (`/(marketing)/events/[id]`). Leading
  slash, no trailing slash, `/` is the root. Every Next marker is a
  legal segment; whitespace, `.`/`..`, `_`-prefixed and over-200-char
  segments are refused. The URL is derived, never stored.
- **`boundary_kind`** — required on `boundary`: `loading`, `error`,
  `not-found`, `template`, `default`, `global-error`. The two error
  kinds require `client_component = true`; `global-error` requires
  `app_path = "/"`.
- **`client_component`** — allowed on `page`, `layout`, `boundary`,
  `component`. `true` emits `"use client"` as the body's first line.
- **`environment`** — required on `service`: `server`, `client` or
  `universal`. Emits `import "server-only"` / `import "client-only"` /
  nothing as the body's first line.

A minimal complete module file (`specs/modules/EventsPage.toml`):

```toml
name = "EventsPage"
role = "page"
app_path = "/events"
description = "Lists upcoming events with title, date and venue."
depends_on = ["EventRepository"]
satisfies = ["REQ-UI-001"]

[[items]]
name = "EventsPage"
kind = "function"
description = "Server Component page rendering the events list."
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

## Route files are derived, not authored

`app/events/page.tsx` is not a module. It is a two-line shim the
analyzer derives from `EventsPage`'s `role` and `app_path`:

```ts
export * from "../../src/events_page";
export { default } from "../../src/events_page";
```

Three forms exist: this one (page, layout, app, most boundaries); the
same with `"use client";` on top (`error`, `global-error` — Next checks
the directive in the route file itself); and a one-line `export *` for
`route_handler` (`route.ts` has no default export). Route segment
config (`dynamic`, `revalidate`, `metadata`, `generateStaticParams`,
`generateMetadata`) lives in the body and is carried through `export *`;
a named re-export is a hard build error. Never hand-edit a shim.

## The client/server boundary

Next.js places a module by the import graph. The spec carries the two
facts that decide it, and the emitted first line makes the compiler
enforce them:

| spec | first line of the body | compiler effect |
|---|---|---|
| `client_component = true` | `"use client";` | everything it imports joins the client graph |
| `environment = "server"` | `import "server-only";` | an import from the client graph fails `next build` |
| `environment = "client"` | `import "client-only";` | an import from the server graph fails `next build` |
| `environment = "universal"` | *(none)* | documentation only |

There is **no validate-time client/server rule in v1**. Every
wrong chain the design reviews built fails `next build` through those
imports; the fast gate (`next typegen`, `tsc --noEmit`) is blind to
them, so a wrong chain surfaces at the batch gate. The enforcement
holds only when the guard line is present — the conventions make its
emission unconditional, and the cold-follow smoke reads one generated
service body for it. Legal and silent: client component →
`server_action` → server service (the form pattern); server page →
client component → client service.

## Config files: written once, then yours

`specdev-analyzer manifest --write` emits `package.json` from the spec
and, **only when absent**, writes `tsconfig.json`, `next.config.ts`,
`vitest.config.ts`, `playwright.config.ts`, `.gitignore` and
`app/.gitkeep` from the profile templates. After that first write they
are externally owned: never rewritten, never gen-recorded. Delete one
and the next write brings the template back. `next build` rewrites
`tsconfig.json` when Next's keys are missing; the template is authored
in Next's own post-build shape (plus `strict: true`) so the file stays
byte-identical. `vitest.config.ts` aliases `server-only` to an empty
module, because the package throws under Vitest's default export
condition.

`app/` must exist before the first gate: `next typegen` fails when it
is absent and passes on an empty one, and `tsc --noEmit` on a tree with
no inputs fails until typegen has written `next-env.d.ts`.
`app/.gitkeep` is the paired file that guarantees it.

## Gates

Two gates (the registry and the runner that execute them are not yet
shipped):

| gate | scope | steps |
|---|---|---|
| `fast` | module | `pnpm install` → `pnpm exec next typegen` → `pnpm exec tsc --noEmit` → `pnpm exec vitest run --passWithNoTests` |
| `next-build` | batch | `pnpm exec next build` |

The fast gate omits `--frozen-lockfile` on purpose: the lockfile is
brought back in step by the gate, and its hash joins the gate key after
the run. `--passWithNoTests` hides a wrong `include` glob; the template
pins it to `src/**`.

## Source layout

Module names are identifiers (`EventsPage`, `EventRepository`); each
emits to `src/<snake>.ts` or `.tsx` (`[paths] source = "src"`). Tests
sit beside their module as `src/<snake>.test.ts(x)`; e2e tests under
`e2e/`. See the conventions doc's "Generated-file ownership" for what
is never generated or recorded.

## Graph checks shipped

- `unique_qualified_names`, `modules_acyclic`, `requirement_ids_unique`
- `role_dependency` (warning) — role ordering over `depends_on`
- `requirements_have_satisfiers`, `test_requirements_have_verifiers`
- Triad consistency: supersession chain shape, OQ status consistency,
  `codegen_unblocked`

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
- **A guard-line presence check** — the guard import is generator
  output. Trigger: the first generated `service` body found without it.
- **Intercepting-route markers in the derived URL** — legal segments,
  not stripped in reports.

## Extending

Blocks contribute their own **top-level** namespaced tables to modules
(composed through the module schema's `allOf` aggregation); the
`bindings` slot is reserved for *bridge* contributions. Inline blocks
with `/spec-inline`. Framework-specific codegen prose layers via each
block's `codegen/typescript/README.md` on top of the conventions doc.
