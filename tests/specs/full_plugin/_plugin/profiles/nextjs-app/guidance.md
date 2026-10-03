# nextjs-app — implementation guidance

Rules an agent follows when it writes or edits code in a `nextjs-app` package: a Next.js App Router application.
Check every file convention and API against the installed Next.js version, not memory. No check verifies these rules;
`check` verifies the roles, the imports and the tests, and review the rest.

## Files

- One module, one file: `src/<snake_name>.ts`, or `.tsx` for modules that render JSX (`app`, `layout`, `page`,
  `boundary`, `component`). Unit tests sit beside the module; end-to-end tests live under `e2e/`.
- Route files under `app/` are not modules. Each is a two-line shim that re-exports its module body: all exports plus
  the default. Never put logic in a shim. Route segment config is exported from the body; only the all-exports form
  carries it — a named re-export of segment config fails the build.
- Import siblings by relative path, bodies only, never route files. Add the `depends_on` edge before importing.

## Roles

- `app` → `layout` → `page` → `route_handler` → `boundary` → `component` → `server_action` → `service` → `model`;
  dependencies point down or stay in the role.
- `app`: exactly one, the root layout body with global providers and default metadata.
- `page`: a Server Component by default. Await `params` and `searchParams` (they are promises); fetch data in the
  body and pass it to client components as props.
- `route_handler`: one named export per HTTP method, no default export.
- `boundary`: error boundaries start with the client directive.
- `component`: a Server Component unless `client_component = true`.
- `server_action`: starts with the server directive; validates input, calls services, revalidates or redirects, and
  returns typed results instead of throwing to the form.
- `service`: no JSX or React. `model`: pure types and functions; no browser or Node API, no fetch.

## The client/server boundary

- `client_component = true` makes the client directive the first line of code (a leading comment may sit above it).
- A service's `environment` sets its first line of code: `server` imports `server-only`, `client` imports
  `client-only`, `universal` has neither and touches no browser or Node API. Never omit the guard: without it a server
  service imported from the client builds and ships its code, and any secret it reads, to the browser.
- A client page or layout exports no metadata. A universal service that reads `window` builds, then fails on the first
  request of a dynamic route.

## TypeScript

- Strict mode; no `any` in exported surface; `unknown` at boundaries, narrowed before use; type-only imports for
  types; discriminated unions for closed variants.
- Throw typed error subclasses, never strings. No floating promises. Prettier's default format.
- Never ship a "not implemented" body. An unfinished test throws and names the requirement it owes.

## Owned files

- `package.json` is the truth for dependencies. The config files the profile writes once (`tsconfig.json`,
  `next.config.ts`, test configs, `.gitignore`, `pnpm-workspace.yaml`) belong to the project afterwards.
- `.next/`, `node_modules/`, the lockfile and `next-env.d.ts` belong to the tooling. Commit the lockfile.
