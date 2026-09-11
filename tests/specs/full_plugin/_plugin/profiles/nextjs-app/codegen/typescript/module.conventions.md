# nextjs-app: TypeScript module conventions

These conventions describe how a `nextjs-app` profile module spec maps
to TypeScript source in a Next.js 16 App Router application. They are
language-and-architecture-only — framework additions (an ORM client, an
auth library, a component kit) come from the relevant block's
`codegen/typescript/README.md`, layered on top.

Read the framework, do not recall it: Next.js changes file conventions
and APIs between releases. Before emitting an API you have not verified
this session, check it against the installed version
(`pnpm exec next --version`, then the documentation for that version).

## File layout

Each module emits as `src/<snake>.ts` or `src/<snake>.tsx` —
`EventCard` → `src/event_card.tsx`. One module, one file; helpers
private to the module live in the same file, unexported. A module that
renders JSX (`app`, `layout`, `page`, `boundary`, `component`) is
`.tsx`; the others (`route_handler`, `server_action`, `service`,
`model`) are `.ts`. The `<snake>/index.tsx` form is accepted when a
module genuinely needs sibling files (a `component` with a
co-located stylesheet); the plain file is preferred.

Tests live beside the module: `src/<snake>.test.ts` or `.test.tsx`
(Vitest). End-to-end tests live under `e2e/` (Playwright).

**Route files are not modules.** `app/**/page.tsx`, `layout.tsx`,
`route.ts` and the boundary files are *derived files*: two-line shims
that re-export the module body, emitted by the analyzer from the
module's `role`, `boundary_kind` and `app_path`. Never write one by
hand and never put logic in one — the shim carries the body's
`dynamic`, `revalidate`, `generateStaticParams`, `generateMetadata` and
`metadata` exports through `export *`. Route segment config therefore
lives **in the body**, as direct `export const` declarations:

```ts
// src/events_page.tsx — the body
export const dynamic = "force-dynamic";
export const metadata = { title: "Events" };
export default async function EventsPage() { … }
```

```ts
// app/events/page.tsx — derived, never edited
export * from "../../src/events_page";
export { default } from "../../src/events_page";
```

A named re-export of segment config (`export { dynamic } from …`) is a
hard `next build` error; only the `export *` form carries it.

## Role semantics

`module.role` states the module's architectural tier and drives the
`role_dependency` ordering check (upward dependencies warn):

| role             | contains                                                     | may depend on (per the check) |
|------------------|--------------------------------------------------------------|-------------------------------|
| `app`            | the root layout body: `<html>`/`<body>`, global providers, fonts, default `metadata` | anything |
| `layout`         | a nested layout body for one `app_path`                      | layout and below |
| `page`           | the UI for one route segment                                 | page and below |
| `route_handler`  | HTTP handlers (`GET`, `POST`, …) for one `app_path`          | route_handler and below |
| `boundary`       | one auxiliary route file's body (loading, error, not-found, template, default, global-error) | boundary and below |
| `component`      | reusable UI below page level                                 | component, server_action, service, model |
| `server_action`  | `"use server"` mutations and form actions                    | server_action, service, model |
| `service`        | non-UI logic and state: data access, stores, clients         | service, model |
| `model`          | pure types and domain logic                                  | model |

Emission guidance per role:

- **`app`** — exactly one per project. The body is the root layout:
  `export default function RootLayout({ children })` returning
  `<html><body>{children}</body></html>`, plus the default `metadata`
  object and global providers. Its derived file is `app/layout.tsx`.
- **`layout`** — `export default function <Name>({ children })`. A
  nested client layout is legal (`client_component = true`) but must
  export no `metadata`.
- **`page`** — `export default async function <Name>(props)` for a
  Server Component page; `props.params` and `props.searchParams` are
  Promises in Next 16 — `await` them. Fetch data in the body; pass it to
  client components as props.
- **`route_handler`** — named exports per method: `export async function
  GET(request: Request)`. No default export — the derived `route.ts`
  is a one-line `export *`. Return `Response` or `NextResponse`.
- **`boundary`** — one default export. `loading`: a skeleton.
  `error` / `global-error`: `"use client"` first line (required),
  `export default function <Name>({ error, reset })`. `not-found`: a
  static page. `template`: like a layout that remounts per navigation.
  `default`: the fallback for a parallel-route slot.
- **`component`** — named `export default function <Name>(props)`;
  props typed with an exported `interface <Name>Props`. Server
  Component unless `client_component = true`.
- **`server_action`** — first line `"use server"`; `export async
  function <verb><Noun>(formData: FormData | input)`. Validate input,
  call services, `revalidatePath`/`revalidateTag`/`redirect` as the
  spec says. No default export.
- **`service`** — plain exported functions or a class; first line is
  the guard import (below). No JSX, no React imports.
- **`model`** — `export interface` / `export type` / `export const`
  enumerations and pure functions. No `import` of anything outside
  `model` modules and type-only libraries. No `window`, no `process`,
  no `fetch`.

## The client/server boundary: two fields, three first lines

Next.js decides where a module runs by the import graph, not by the
file. Two spec fields drive the three lines that make the compiler
enforce the placement.

**`client_component = true`** (page, layout, boundary, component) emits
`"use client";` as the first line of the body. Everything this module
imports joins the client graph. Do not emit the directive when the
field is absent or false.

**"First line of the body" means the first line of *code*.** The
`// @spec` regen-pointer comment (and an SPDX header, when the project
declares a license) sits **above** the directive or the guard import,
and that is correct — a leading comment does not stop Next from seeing
the directive. Measured on Next 16.3.4 / Turbopack: a client component
whose first two lines are `// @spec …` then `"use client";` builds
(exit 0) and its `useState` works; the same file with the directive
removed and the comment kept fails the build with *"You're importing a
module that depends on `useState` into a React Server Component
module"*. Do not "fix" the ordering by moving the comment below the
directive, and do not drop the comment to get the directive onto
line 1.

**`environment`** (service only) emits the guard import as the first
line:

| `environment` | first line                | effect                                                                 |
|---------------|---------------------------|------------------------------------------------------------------------|
| `server`      | `import "server-only";`   | any import from the client graph fails `next build`                    |
| `client`      | `import "client-only";`   | any import from the server graph fails `next build`                    |
| `universal`   | *(nothing)*               | documentation only — the body must contain no browser and no Node API  |

**Emit the guard line without exception.** The compiler enforces the
chain only when the line is present: a `server` service without it,
imported by a client component, builds and ships its code (and any
secret it references) to the browser (measured, Next 16.3.4). Nothing
before `next build` checks the line is there, and `next build` checks
only that a *present* line is on the right side. The line is the whole
enforcement.

What builds and what fails (all measured on Next 16.3.4; every wrong
chain passes `next typegen` and `tsc --noEmit` and fails only at
`next build`):

- **fails**: client component → (any chain of components, models,
  universal or client services) → `server` service; server page →
  `client` service directly; server page → plain component → `client`
  service; `server_action` → `client` service; `model` → `client`
  service imported from a server page.
- **builds**: client component → `server_action` → `server` service
  (the canonical form pattern — the action is a server boundary);
  server page → client component → `client` service; client page →
  plain component → `client` service; client page → `client` service.

Two limits the compiler does not cover: a `universal` service that
gains a `window` read builds, and a dynamic route that imports it
returns HTTP 500 on the first request (static routes fail at build);
and a `client_component` page or layout that exports `metadata`
passes `typegen` and `tsc` and fails `next build` with a `$$typeof`
error. State both in the module description when they are near.

## Item emission

`module.items[]` carries the module's contractual surface. The rule is
one line:

> An item is an **exported, contractual code symbol, identified by its
> qualified name, described in prose — never by signature.**

TypeScript specifics:

- Visibility is `export`. A non-exported declaration is never an item.
  A class member is public unless `private`, `protected` or
  `#`-prefixed; members qualify with `.` — `<Class>.<member>`.
- The default export of a `page`, `layout`, `boundary`, `component` or
  `app` module is one item, `kind = "function"`, named after the
  module (`EventsPage`). Route segment config exports (`dynamic`,
  `revalidate`, `metadata`, `generateStaticParams`, `generateMetadata`)
  are items only when a requirement names them.
- `route_handler` method exports are items (`GET`, `POST`), one each.
- Constructors, fields, interface members and overload signatures are
  not items (the language schema lists the exclusions).
- Prose-first: each item carries a `description` of what it does, NOT
  a signature mirror. Never name parameters, return types or generics.

Example — a page module's surface:

```toml
[[items]]
name = "EventsPage"
kind = "function"
description = "Lists upcoming events with title, date and venue."
satisfies = ["REQ-UI-001"]
```

A public export left out of `items[]` still compiles but shows as
`in_source_only` in `/spec-sync-items`. Reconcile by authoring the item
(with `satisfies`) or un-exporting it.

## Test items

An `[[items]]` entry with `test = true` is a test: `kind = "function"`,
`verifies = [...]` (pointing at `nature = "test"` requirements) in place
of `satisfies`, and a `test_kind` driving emit location:

| `test_kind`   | Emit location                                                        |
|---------------|----------------------------------------------------------------------|
| `unit`        | a `test()` case in `src/<snake>.test.ts` (Vitest)                    |
| `component`   | a `test()` case in `src/<snake>.test.tsx` rendering the component    |
| `e2e`         | a `test()` case in `e2e/<snake>.spec.ts` (Playwright, `pnpm run e2e`) |

A `service` with `environment = "server"` is testable under Vitest: the
profile's `vitest.config.ts` aliases `server-only` to an empty module.
Async Server Component pages have no unit-test seam; cover them with
`e2e`.

When the verification context does not carry enough to write a real
test, emit a failing marker, never a vacuous pass:

```ts
test("verifies REQ-TEST-001", () => {
  throw new Error("verifies REQ-TEST-001: criteria not yet specified");
});
```

## Cross-module dependencies

`module.depends_on[]` does not directly emit code — imports appear as
the body is generated. Import siblings by relative path with no
extension: `import { listEvents } from "./event_repository";`. Do not
import a module the spec does not list in `depends_on` — add the edge
first. Import route files never: a body imports bodies.

## External npm dependencies

`module.external_deps[]` declares npm packages the module needs. Each
entry is an NpmReference — `package` required; exactly one source key
(`git`/`path`/`link`/`workspace`) or none for the registry; registry
deps require `version`; `test_only = true` routes to `devDependencies`.
The union across modules, architecture and inlined blocks/bridges is
what the manifest step emits — package.json is OUTPUT, never
hand-maintained. The framework, React, the two guard packages, the type
checker, the test runners and prettier are declared once at
architecture level, not per module.

## TypeScript style

- `strict` is on. No `any` in exported surface; `unknown` at
  boundaries, narrowed before use.
- `import type` for type-only imports; `interface` for object shapes
  that may be extended, `type` for unions and aliases.
- Discriminated unions for closed variants; `satisfies` for
  configuration objects.
- Errors: throw typed `Error` subclasses (one per failure family);
  never throw strings. Server actions return typed results to the form,
  they do not throw across the boundary.
- Async: `async`/`await`; no floating promises.
- Naming: types and components `PascalCase`, functions and variables
  `camelCase`, files `snake_case.ts(x)`.
- Formatting is prettier's default (double quotes, semicolons); the
  derived route files are written in that shape so a project without a
  `.prettierrc` never sees them as edited.

## Refusal rule

Emitting `throw new Error("not implemented")`, a `TODO` body, or an
empty function as an implementation is **refused output** — the same
rule as every other language's stub markers. If the spec lacks the
information to implement an export, stop and report which module and
item is underspecified. The single exception is the test-item failing
marker above, which runs and fails loudly.

## Generated-file ownership

Four generators may touch a Next.js tree; only two are spec-owned.

| Generator                      | Owns                                                                                  | gen-recorded? |
|--------------------------------|---------------------------------------------------------------------------------------|---------------|
| `/spec-gen` (LLM step)         | `src/**` bodies, `src/**/*.test.ts(x)`, `e2e/**`                                      | always        |
| `specdev-analyzer manifest`    | `package.json`                                                                        | always        |
| `specdev-analyzer derive`      | the route shims it writes under `app/` (`page.tsx`, `layout.tsx`, `route.ts`, boundary files) | always, as `derived` rows |
| profile `paired_files` (written once when absent) | `tsconfig.json`, `next.config.ts`, `vitest.config.ts`, `playwright.config.ts`, `.gitignore`, `app/.gitkeep` | **never** |
| Next.js / pnpm / tooling       | `node_modules/`, `.next/`, `pnpm-lock.yaml`, `next-env.d.ts`, `*.tsbuildinfo`, the `AGENTS.md`/`CLAUDE.md` blocks `next dev` writes | **never** |

Gen owns the derived files it writes, **not `app/**`**: a hand-written
file under `app/` is externally owned and invisible to `gen-drift`; a
hand-written file at a derived target blocks that shim and is reported,
never overwritten silently. `next build` rewrites `tsconfig.json` on
its first run when Next's keys are missing, which is why that file can
only ever be written once and never recorded.
