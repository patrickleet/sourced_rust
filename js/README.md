# `@hops-ops/distributed`

The generated, end-to-end typed client for
[Distributed](https://github.com/hops-ops/distributed) services.

Rust table, relationship, role, and command definitions produce one authorized
client surface. `distributed client` combines that surface with application GraphQL
documents and emits typed operations, live companions, route-load plans, and
commands. This package executes those artifacts through one normalized,
causally consistent browser replica.

The intended application experience is:

```ts
const todos = Todos.use();
const commands = useCommands();
await commands.todo.create({ title });
```

Reads populate the replica on demand. Every UI consumer reads that same
replica. Generated optimistic command effects update it synchronously, and
server-issued record/index clocks reconcile the authoritative result without
application-authored cache policies.

## Install

```bash
npm install @hops-ops/distributed graphql
```

The package is ESM-only and requires Node 20 or newer for server-side use. It
also runs in modern browsers. Svelte 5 and React are optional peers used only
by the `/sveltekit` and `/react` entry points, respectively.

## Generate the client surface

The service, not the browser, owns authorization and GraphQL semantics:

```bash
distributed client-manifest > target/distributed-client.json

distributed client \
  --manifest target/distributed-client.json \
  --role user \
  --documents 'src/**/*.graphql' \
  --out src/lib/generated/distributed
```

Use `--surface <name>` for a named application surface. CI can append `--check`
to validate that committed artifacts are current without rewriting them.

A co-located route document can opt into SSR and live continuation:

```graphql
query Todos @load @live {
  todos(order_by: [{ status: asc }, { todo_id: asc }]) {
    todo_id
    title
    status
  }
}
```

Generation validates the document against the selected role/application,
injects wire-only identity and revision fields, and emits:

- an exact typed operation and optional live companion;
- normalization, identity, relationship, filter, order, and pagination plans;
- the closed variable codec used before cache lookup or transport;
- a static `@load` route registry;
- an SSR-safe SvelteKit wrapper with static operation bindings and tree-local
  client/command access;
- a nested command tree with input defaults, optimistic effects, and causal
  confirmation contracts;
- an exact schema, protocol, and client-surface binding.

Unsupported or unprovable behavior fails during generation. The runtime does
not parse GraphQL documents, guess cache keys, or infer mutation effects.

## SvelteKit

Install Svelte and describe each generated authorization surface once:

```bash
npm install svelte
```

```js
// distributed.config.js
const serviceManifestArgs = [
  'client-manifest',
  '--manifest-path',
  '../service/Cargo.toml',
  '--package',
  'service'
];

export const distributedClients = [
  {
    module: '$distributed',
    manifest: { args: serviceManifestArgs },
    surface: 'e2e-ui',
    documents: ['src/routes/(app)/**/*.graphql'],
    out: 'src/lib/generated/distributed'
  },
  {
    module: '$distributed/admin',
    manifest: {
      args: [
        ...serviceManifestArgs,
        '--entrypoint',
        'service::distributed_admin_client_surface'
      ]
    },
    surface: 'e2e-ui-admin',
    documents: ['src/routes/admin/**/*.graphql'],
    out: 'src/lib/generated/distributed-admin'
  }
];

export const distributedViteOptions = {
  clients: distributedClients
};
```

The common and elevated document sets must not overlap. The route group above
keeps ordinary application documents out of the admin tree; each trust boundary
has its own Rust manifest entrypoint, generated directory, virtual module, and
request-local replica. A single-surface application can omit the second entry.

The Vite integration runs `distributed client` at startup/build, watches GraphQL
documents, stages all surfaces, commits a rollback-capable multi-output
transaction, then triggers one reload. It exposes the generated Svelte wrapper
through the configured virtual module:

```ts
// vite.config.ts
import { sveltekit } from '@sveltejs/kit/vite';
import {
  distributedGraphqlProxy,
  distributedSvelteKit
} from '@hops-ops/distributed/sveltekit/vite';
import { defineConfig } from 'vite';
import { distributedViteOptions } from './distributed.config.js';

export default defineConfig({
  plugins: [distributedSvelteKit(distributedViteOptions), sveltekit()],
  server: {
    proxy: distributedGraphqlProxy('http://127.0.0.1:8791')
  }
});
```

Give SvelteKit’s language tools the identical aliases:

```js
// svelte.config.js
import {
  distributedSvelteKitAliases
} from '@hops-ops/distributed/sveltekit/vite';
import {
  distributedClients,
  distributedViteOptions
} from './distributed.config.js';

export default {
  kit: {
    alias: distributedSvelteKitAliases({
      cwd: distributedViteOptions.cwd,
      clients: distributedClients
    })
  }
};
```

One-shot scripts use the same configuration and transaction:

```js
import {
  checkDistributedSvelteKit,
  generateDistributedSvelteKit
} from '@hops-ops/distributed/sveltekit/vite';
import { distributedViteOptions } from './distributed.config.js';

await generateDistributedSvelteKit(distributedViteOptions);
await checkDistributedSvelteKit(distributedViteOptions); // never writes
```

Create one request-local server replica in the root layout:

```ts
// src/routes/+layout.server.ts
import {
  createDistributedSvelteKitServer
} from '@hops-ops/distributed/sveltekit';
import {
  DISTRIBUTED_ROUTE_OPERATIONS
} from '$distributed';

const distributed = createDistributedSvelteKitServer({
  routes: DISTRIBUTED_ROUTE_OPERATIONS,
  getSession: ({ locals }) => locals.auth(),
  getRole: (session) => roleFromSession(session)
});

export const load = distributed.load;
```

The browser layout installs one client in Svelte context for the current
authorization lifecycle. The generated module retains no client singleton:

```ts
// src/routes/+layout.svelte
import { browser } from '$app/environment';
import {
  createPageDataSessionSource
} from '@hops-ops/distributed/sveltekit';
import { provideDistributed } from '$distributed';

let { data, children } = $props();
const pageData = createPageDataSessionSource(data);

const client = provideDistributed({
  browser,
  session: pageData.session,
  ...(data.distributed !== undefined &&
  data.distributedAuthority !== undefined
    ? {
        hydration: data.distributed,
        authority: data.distributedAuthority
      }
    : {})
});

$effect(() => pageData.set(data));
```

Route components import only their generated surface. Static operation wrappers
resolve the nearest tree-local client when used:

```ts
// src/routes/todos/+page.svelte
import { Todos, useCommands } from '$distributed';

const todos = Todos.use(); // generated @live attaches automatically
const commands = useCommands();

await commands.todo.create({ title: 'Ship it' });
// $todos.data, $todos.status, $todos.pending
```

When the Rust command declaration supplies a UUIDv7, ULID, or literal input
default, generation makes that field optional and the runtime fills it exactly
once. Components do not generate IDs or maintain optimistic/cache recipes.

`@load` results are normalized on the server, dehydrated, and restored in the
browser without a duplicate first request. Hydration cannot authorize itself:
the server sends a separate authority value, and the adapter requires both
values to match. Session, token, tenant, or role changes abort HTTP and live
work, discard the old generation, and reconnect under server-issued scope.

Confirmed records and indexes under an active scope stay until auth/scope
change, stale+revalidate, or a newer authoritative write. Same-scope soft
navigation merges a route SSR seed into the warm client and does **not** wipe
keys the seed omitted (a page dehydrate is only the subset for that route).

Use a separate generated surface and replica for elevated routes. A normal
client cannot import or mix admin artifacts. Configure it as a separate virtual
module such as `$distributed/admin` and provide it only in the elevated layout.

## Framework-neutral replica

Other frameworks can bind the same core directly:

```ts
import {
  createDistributedReplica,
  createReplicaGraphqlTransport
} from '@hops-ops/distributed/replica';
import { Operation_Todos } from './generated/distributed/index.js';

const transport = createReplicaGraphqlTransport({
  getUrl: () => '/graphql',
  getAuth: () => ({ accessToken: session.accessToken })
});
const replica = createDistributedReplica({ transport });
const todos = replica.watch(Operation_Todos, {}, { live: true });

const unsubscribe = todos.subscribe((snapshot) => {
  render(snapshot.data, snapshot.status);
});
```

`watch()` reads synchronously, fetches only missing or stale projections,
deduplicates work, and optionally maintains the generated live operation.
`read()` is side-effect-free. `dehydrate()` and `hydrate()` transfer confirmed
state without exposing a public storage schema. Cold `hydrate` seeds an empty
client; warm same-scope `hydrate` merges so soft navigation cannot discard
confirmed session data the next route did not re-dehydrate.

The replica stores normalized records and exact argument-sensitive indexes,
not GraphQL response blobs. Generated selection metadata reconstructs each
operation result from that shared state, so a detail read, list read, live
frame, or optimistic command can update every affected view in one transaction.

## Commands and optimistic UI

Generated `createCommands` binds the service-owned command artifacts to the
same replica and GraphQL transport. A command call:

1. validates and freezes its typed input;
2. fills generated UUIDv7, ULID, or literal defaults exactly once;
3. applies the generated optimistic effect transaction (from `.applies` /
   portable mutation IR — works for **Eventual and Direct** when fields are known);
4. dispatches the exact compiler-owned mutation;
5. keeps ambiguous commits recoverable by command ID;
6. confirms or rejects only its own optimistic layer;
7. retires the layer on the path that placement allows:
   - **Eventual** — wait for projection obligations (event handler ran
     async; there is no authoritative row on the command response);
   - **Atomic / Direct** — normalize the **returned** row
     (`confirmDirectProjection`) before the call settles. The server waited in
     the command handler because it could; an event handler cannot.

Applications do not provide list targets, merge functions, mutation update
callbacks, board simulators, or invalidation maps. If the compiler cannot prove
safe maintenance, the generated plan marks the affected projection stale and
the replica performs one deduplicated revalidation.

Callers may bound their own causal wait without inventing a rollback:

```ts
const receipt = await commands.todo.create(
  { title: 'Ship it' },
  { signal: AbortSignal.timeout(5_000) }
);
await receipt.projected;
```

Before acceptance the signal cancels dispatch. After finite acceptance it
rejects only that caller's `receipt.projected` wait; the optimistic layer and
internal causal tracking remain active, and `receipt.status()` stays available.

## React

Install React and use the optional adapter over an application-owned replica:

```tsx
import {
  DistributedProvider,
  useDistributedQuery
} from '@hops-ops/distributed/react';
import { Operation_Todos } from './generated/distributed/index.js';

function TodosView() {
  const todos = useDistributedQuery(Operation_Todos, {}, { live: true });
  return todos.complete
    ? todos.data.todos.map((todo) => <div key={todo.todo_id}>{todo.title}</div>)
    : null;
}

root.render(
  <DistributedProvider replica={replica}>
    <TodosView />
  </DistributedProvider>
);
```

The adapter is only a `useSyncExternalStore` bridge. It does not add another
cache, transport, auth lifecycle, or command path. For SSR, create one replica
per request and hydrate only under the same authoritative scope.

## Persistence and diagnostics

The default replica is memory-only. Optional IndexedDB persistence is explicit,
confirmed-state-only, and governed by generated/application model policy.
Optimistic layers, command inputs, credentials, cache authority, and live
connections are never persisted as replica data.

Diagnostics are also opt-in:

```ts
import {
  createReplicaDiagnostics
} from '@hops-ops/distributed/diagnostics';

const diagnostics = createReplicaDiagnostics();
const replica = createDistributedReplica({ transport, diagnostics });
const commands = createCommands(replica, transport, { diagnostics });
```

Snapshots explain operation artifacts, normalized records, index coverage,
optimistic layers, causal receipts, revalidation, response fences, and garbage
collection. Defaults pseudonymize identities and omit values, arguments,
credentials, trusted presets, raw command inputs, and cache scope. Revealing
additional development detail requires an in-process capability plus an
explicit redactor.

## Protocol and security boundaries

Every accepted artifact and response is protocol v1 and carries an exact
schema/client-surface binding. Every response is also bound to a server-issued
cache scope, operation ID, and trusted-preset inventory; any supplied record
clocks or index vector are bound to that same scope. Missing, malformed,
stale-schema, or cross-surface evidence fails closed. An exact authorized
payload without a safely comparable index vector may render, but it cannot
advance index clocks/vectors, observations, live resume, or optimistic
confirmation. Independently valid record clocks remain usable.

OIDC credentials authorize transport requests; decoded client claims never
create cache authority. GraphQL remains the API and command proxy layer, while
the service's SQL read models remain authoritative.

Normative architecture and API decisions live in the Distributed GitKB,
including `specs/query-layer/v1/cache-engine`. They are intentionally not
duplicated as decision documents in this package.

## Public entry points

- `@hops-ops/distributed` — GraphQL HTTP/WebSocket, auth, and protocol
  primitives.
- `@hops-ops/distributed/replica` — replica, GraphQL transport, generated
  command runtime, query-plan helpers, and optional persistence.
- `@hops-ops/distributed/diagnostics` — redacted support snapshots and artifact
  inspection.
- `@hops-ops/distributed/sveltekit` — Svelte stores, SSR route loading,
  hydration, auth lifecycle, and tree-local generated bindings.
- `@hops-ops/distributed/sveltekit/vite` — Node-only one-shot/check/watch
  generation, virtual module aliases, and GraphQL HTTP/WebSocket proxy helpers.
- `@hops-ops/distributed/react` — provider and query hook over the same replica.

All other subpaths are private and blocked by the package export map.

## Pre-release clean break

The earlier pilot API and persistence format are intentionally unsupported.
There is no `QueryCache`, `CacheTarget`, `ListMergeSpec`, `target/at/by`
addressing, manual cache-policy map, resource wrapper, document store, legacy
command pipeline, or package-owned codegen executable.

To move an existing pilot application:

1. rerun `distributed client` and import its operation/command artifacts;
2. compose one replica through the framework adapter or core transport;
3. remove handwritten cache targets, merge/update callbacks, and invalidation
   policies;
4. discard prior browser cache and SSR payloads rather than migrating them.

Only protocol-v1 generated artifacts and server envelopes are accepted.

## Verification and release

```bash
npm ci
npm run quality
npm run release:dry-run
```

`quality` typechecks generated consumers, runs behavior and adapter suites,
packs and installs the real tarball into clean consumers, verifies bundle
boundaries, and runs `publint`. `release:dry-run` exercises npm's publish
payload without publishing.

Version tags (`vX.Y.Z`) publish with npm provenance through GitHub Actions
trusted publishing. The package currently uses `UNLICENSED` because the
repository has no top-level license file; changing that is an explicit
maintainer decision.
