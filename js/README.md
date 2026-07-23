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

The Distributed service exports a role-selected client manifest; the package
CLI turns that service-owned contract into typed app-owned artifacts:

```bash
distributed-gen-commands \
  --manifest src/lib/api/commands.manifest.json \
  --commands src/lib/api/commands.generated.ts \
  --operations src/lib/api/commands.operations.gql \
  --policies src/lib/api/commands.policies.generated.ts
```

Client manifest v6 carries nested command types, scalar codecs, the exact
compiler-owned mutation documents, and one exact command-status operation. The
generator validates operation hashes and emits those bytes verbatim; the
runtime never guesses a status query or mutation field. Legacy command manifest
v1 remains usable as an explicitly non-causal fallback.

The generated module exports standalone functions and one `bindCommands`
function. Bind once, then call commands without repeating transport or auth:

```ts
const commands = bindCommands(gql, { cache, policies: commandPolicies });

const result = await commands.todosCreate(
  { todo_id: crypto.randomUUID(), title: 'Ship it' },
  {
    optimistic: {
      targets: [list.target('todos', 'todo_id')],
      row: optimisticTodo
    }
  }
);

if (result.errors?.length) {
  showCommandErrors(result.errors);
} else {
  await result.receipt?.projected;
}
```

When commands are bound with a cache, the pipeline performs optimistic apply,
bounded same-ID transport recovery, result-policy handling, UI effects, and
explicit reconciliation. A response loss preserves optimism because the
command may have committed; an authoritative rejection rolls it back. Without
a cache, commands execute directly.

Generated causal results include a durable `receipt`. Its lazy `projected`
promise resolves only after the server proves projection completion, and
rejects with a typed safe error for rejection, projection failure, expiry,
scope/schema drift, abort, or deadline. Accepted commands with no finite
projection contract intentionally expose no `projected` promise. Pass a UUIDv7
`commandId` in call options when independent callers need to submit the same
identity; the runtime does not persist command identities or claim live resume.

Projection evidence does not fabricate a browser row. The runtime never invents
a complete projection from an acknowledgement, partial fact, or status result;
apps use their generated query/live/refetch path to materialize authoritative
read-model data.

## Compiler-backed replica artifacts

`dctl client` emits framework-neutral query, live, and prepared-command modules
from the role/application-selected Rust manifest and co-located GraphQL
documents. Generated query artifacts carry exact result/variable types,
normalization and selection plans, portable filter/order facts, pagination
fallbacks, a schema/protocol binding, and the variable codec used for cache
identity and transport.

```ts
import { createDistributedReplica } from '@hops-ops/distributed/replica';
import { Todos } from './generated/distributed/index.js';

const replica = createDistributedReplica({ transport });
const todos = replica.watch(Todos, { where: { completed: { _eq: false } } }, {
  live: true
});

const unsubscribe = todos.subscribe((snapshot) => render(snapshot));
```

Variables are validated and canonicalized before lookup or I/O, so GraphQL
singleton-list forms, IDs, object key order, and omitted versus explicit-null
values have one deterministic identity. One replica accepts artifacts from one
generated schema surface; mixing legacy, stale-schema, or elevated-surface
artifacts fails before cached data can be observed or a request can be sent.
Framework adapters consume this API rather than owning normalization, command,
auth, or protocol behavior.

Variable codec v2 also carries the service's exact filter execution limits.
Boolean-list and IN-list widths are checked after GraphQL singleton coercion,
while filter depth follows server semantics: only `_and`/`_or` children,
`_not`, and relationship predicates enter a child. Root filters begin at depth
zero; relationship selection and aggregate arguments inherit their model-edge
depth. Compiler-emitted `filterBaseDepth` and `maxItems` constraints preserve
the most restrictive use when one variable appears more than once. A separate
64-level input traversal cap remains in place as a client-runtime safety bound.

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
