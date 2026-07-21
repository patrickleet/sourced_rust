# `@hops-ops/distributed`

Typed GraphQL transport, document cache, generated-command runtime, and a thin
SvelteKit adapter for [Distributed](https://github.com/hops-ops/distributed)
services.

The package stays intentionally small: GraphQL remains the wire format, service
manifests remain the source of command types, and the cache is a client-side
projection aid rather than a system of record.

## Install

```bash
npm install @hops-ops/distributed graphql
```

The package is ESM-only and requires Node 20 or newer for server-side use. It
also works in modern browsers. The SvelteKit entry point uses structural load
types, so it does not pull a second SvelteKit/Vite dependency graph into apps.
HTTP uses the runtime's global `fetch`; subscriptions require a global
`WebSocket` or an injected implementation in runtimes that do not provide one.

## Core client

```ts
import { createGraphqlClient, defineResource, QueryCache } from '@hops-ops/distributed';

const cache = new QueryCache();
const gql = createGraphqlClient({
  getUrl: () => '/graphql',
  getAuth: () => ({ accessToken: session.accessToken }),
  cache
});

const todos = defineResource({ query: TodosDocument });
const result = await gql.request(todos.query);
const stop = gql.subscribe(ChatMessagesLiveDocument, {
  onNext: (payload) => console.log(payload)
});
```

HTTP and WebSocket calls share the same URL and auth suppliers. Bearer auth is
preferred; `x-user-id`/`x-role` are supported only as a local-development
fallback. When a cache is configured, query and subscription results write
through under the same document-and-variables key. Mutation results are handled
by the command pipeline instead of being mistaken for read-model truth.

## Generated commands

The Distributed service exports `commands.manifest.json`; the package CLI turns
that service-owned manifest into typed app-owned artifacts:

```bash
distributed-gen-commands \
  --manifest src/lib/api/commands.manifest.json \
  --commands src/lib/api/commands.generated.ts \
  --operations src/lib/api/commands.operations.gql \
  --policies src/lib/api/commands.policies.generated.ts
```

Manifest version 1 generates scalar command fields (`String`, `ID`, `Boolean`,
`Int`, `Float`, `BigInt`, and `JSON`). The generator rejects nested objects,
enums, and custom scalars with a field path instead of emitting incomplete
TypeScript types or invalid GraphQL selections; those shapes require explicit
type metadata in a future manifest version.

The generated module exports standalone functions and one `bindCommands`
function. Bind once, then call commands without repeating transport or auth:

```ts
const commands = bindCommands(gql, { cache, policies: commandPolicies });

await commands.todosCreate(
  { todo_id: crypto.randomUUID(), title: 'Ship it' },
  {
    optimistic: {
      targets: [list.target('todos', 'todo_id')],
      row: optimisticTodo
    }
  }
);
```

When commands are bound with a cache, the pipeline performs optimistic apply,
one command request, rollback on failure, result-policy handling, UI effects,
and explicit reconciliation. Without a cache, commands execute directly. The
runtime never invents a complete projection row from an acknowledgement or
partial fact.

## SvelteKit

Configure generated commands once in an app-local composition module:

```ts
// src/lib/gql/index.ts
import { createUseGraphql } from '@hops-ops/distributed/sveltekit';
import { bindCommands, COMMANDS } from '$lib/api/commands.generated';
import { commandPolicies } from '$lib/api/commands.policies.generated';

export const useGraphql = createUseGraphql<typeof COMMANDS>({
  bindCommands,
  policies: commandPolicies
});
export { fx } from '@hops-ops/distributed';
```

Pages keep the pilot’s compact API:

```ts
const gql = useGraphql(() => data);
const list = gql.store({
  document: TodosDocument,
  initialData: { todos: data.todos },
  list: { at: 'todos', by: 'todo_id' },
  select: (value) => value.todos
});

await gql.commands.todosComplete({ todo_id }, {
  optimistic: {
    targets: [list.target('todos', 'todo_id')],
    row: { todo_id, status: 'completed' }
  }
});
```

`createLoadQuery` injects app-owned Auth.js locals, role mapping, and the private
API origin for SSR. `distributedGraphqlProxy(apiOrigin)` configures both HTTP
and WebSocket Vite proxying at `/graphql`.

## Public entry points

- `@hops-ops/distributed` — client, transport, auth, resources, document store,
  cache, and common command APIs.
- `@hops-ops/distributed/cache` — explicit cache and command-pipeline surface.
- `@hops-ops/distributed/commands` — generic generated-command runtime.
- `@hops-ops/distributed/codegen` — manifest generator API for tooling.
- `@hops-ops/distributed/sveltekit` — page auth, browser composition, SSR load,
  and Vite proxy helpers.

Unsupported internal paths are intentionally blocked by the package `exports`
map. A future React adapter should consume the framework-neutral root instead
of copying transport, identity, or cache behavior.

## What the app still owns

- command manifests, generated commands/policies, and GraphiQL operations;
- role SDL plus generated query/subscription document types;
- route `.gql` files, domain resources, UI, and optimistic row contents;
- Auth.js/OIDC provider configuration, role mapping, and private environment
  variable names.

## Verification and release

```bash
npm ci
npm run quality
```

`quality` typechecks and builds the package, runs behavioral tests, packs the
real tarball, installs it into a clean temporary consumer, typechecks both root
and SvelteKit imports, and runs a runtime import smoke. The e2e-ui fixture is a
separate integration consumer.

Version tags (`vX.Y.Z`) set the nested package version and publish with npm
provenance through GitHub Actions trusted publishing. The npm trusted publisher
must be configured for `hops-ops/distributed`, the exact release workflow, and
its protected `npm` environment. Creating the scoped package for the first time
requires a one-time authenticated bootstrap publish; a release preflight blocks
all registry writes until maintainers explicitly mark that setup ready. No
long-lived npm token is used afterward.

The Rust workspace declares its crates as MIT licensed, but the repository
currently has no top-level license file. Package metadata therefore deliberately
uses `UNLICENSED`; choosing and adding an npm-package license is a maintainer
decision, not an implicit packaging change.
