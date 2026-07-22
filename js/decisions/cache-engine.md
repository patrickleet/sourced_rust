# Private cache engine: purpose-built sparse record/link/index store

- Status: accepted
- Date: 2026-07-22
- Scope: `tasks/graphql-qs-client-replica-2`

## Decision

Use the purpose-built `CacheEngine` in `src/internal/cache-engine.ts`. Keep the
interface and implementation private to the framework-neutral replica. Do not
ship Apollo Client as a production dependency or expose either candidate's types
from package entry points.

Apollo Client 4 remains a useful behavioral reference. Its public cache API
already provides normalized entities, aliases/fragments, argument-sensitive
fields, sparse diffs, watches, batching, named optimistic layers, GC, and base-
only extraction. It is not selected because its optimistic layer stack follows
creation order: when later layer B is confirmed and removed while earlier layer A
is still pending, A becomes visible again above B's newly written base value.
Distributed's causal contract forbids that older visible state.

The executable comparison in `tests/cache-engine-conformance.test.mjs` records
the behavior without private inspection:

1. base → optimistic A → optimistic B renders B;
2. one public `cache.batch({ update, removeOptimistic: "B" })` writes B's
   projection and removes B atomically;
3. the next optimistic read renders A, not the confirmed B projection;
4. only removing A reveals B's projection.

Adding source-revision and tombstone fencing around Apollo would also require a
second metadata store ahead of every write. Rebuilding/suppressing older layers
after out-of-order confirmation would require retaining Distributed operations
and reconstructing Apollo layers, or depending on its private optimistic-store
chain. At that point Apollo would be a second storage engine beneath the causal
engine rather than the implementation of the private seam.

## Conformance result

| Capability | Purpose-built | Apollo 4 public API |
| --- | --- | --- |
| normalized records, links, exact argument indexes | pass | pass |
| alias/fragment identity and partial field presence | pass via generated-like normalization plan | pass via GraphQL cache APIs |
| missing distinct from present `null` | pass | pass |
| one observer broadcast per batch | pass | pass |
| named layer survives acceptance/stale base traffic | pass | pass while the layer remains |
| base write + named-layer removal is atomic | pass | pass |
| later layer confirms before an older layer | pass with per-dependency causal floors | **fail: older layer becomes visible** |
| revision/tombstone stale-resurrection fence | pass | **absent; stale arrival rewrites/resurrects** |
| confirmed-only SSR extract/restore | pass | pass with `extract(false)` |
| GC from indexes/retained roots through links | pass | available |

The purpose-built engine stores sparse authoritative records, relationship links,
exact indexes, and ordered optimistic operation layers. Confirmation advances a
field/index causal floor before removing its layer. An older pending layer stays
tracked for its eventual receipt, but operations superseded by the confirmed
later command cannot reappear. A later still-pending layer remains visible above
an earlier confirmation. Tombstones are retained as revision fences, including
across extraction/restoration.

## Bundle and dependency evidence

Reproduce with:

```bash
npm run measure:cache-engines
```

Measurement uses esbuild 0.25.12, browser ESM targeting ES2022, minification,
and gzip level 9. Apollo version is 4.2.7. The Apollo entry imports
`InMemoryCache` from the cache subpath; the purpose-built entry exports the
private factory and exact-index-key helper.

| Candidate | Minified bytes | Minified+gzip bytes | gzip delta over baseline | bundled modules |
| --- | ---: | ---: | ---: | ---: |
| no-cache baseline | 29 | 49 | 0 | 1 |
| purpose-built | 13,066 | 4,036 | 3,987 | 2 |
| Apollo `InMemoryCache` | 94,152 | 30,193 | 30,144 | 458 |

Both deliberately unused candidate imports produce the exact 29-byte/49-byte
baseline, proving the measured configuration tree-shakes either engine when no
replica path uses it. The selected runtime adds no production dependency. The
Apollo spike is dev-only and resolves `@apollo/client`, its `rxjs` dependency,
and the already-required `graphql` package.

## Public API and declaration boundary

The Apollo comparison uses documented public methods only: `writeQuery`,
`watch`, `batch({ optimistic })`, `batch({ removeOptimistic })`, `evict`, and
`extract`. Apollo documents that `batch` broadcasts once, a string `optimistic`
creates a named layer, `removeOptimistic` can accompany a root write atomically,
and `extract(false)` omits optimistic data:

- <https://www.apollographql.com/docs/react/caching/cache-interaction#using-cachebatch>
- <https://www.apollographql.com/docs/react/api/cache/InMemoryCache#batch>
- <https://www.apollographql.com/docs/react/api/cache/InMemoryCache#extract>
- <https://www.apollographql.com/docs/react/caching/cache-configuration>

No Apollo private member is read or invoked. The test suite scans every emitted
`.d.ts` file for Apollo module references and cache/client/link type names. The
private engine file is not present in the package `exports` map, and no vendor
type appears in public declarations.

## Replacement triggers

Reconsider Apollo as the implementation only if all of the following are true:

- documented public APIs can keep an earlier same-entity layer from reappearing
  after a later layer confirms, without reconstructing the optimistic store;
- documented public APIs provide revision/tombstone write guards, or those guards
  can be expressed without duplicating the normalized record/index store;
- the complete Distributed conformance harness passes unchanged, including all
  confirmation/rejection orders and exactly-once observer batches;
- the measured production-core/dependency tradeoff is accepted explicitly.

Reconsider the purpose-built engine if profiling shows its whole-view selector
comparison or materialization cost is material in realistic replicas. Preserve
the seam and conformance suite; optimize dependency tracking/materialization or
swap the private implementation without changing generated artifacts or public
client APIs.

## Verification

```bash
npm run check
npm run build
node --test tests/cache-engine-conformance.test.mjs
npm run measure:cache-engines
```

The conformance test covers alias/fragment normalization plans, by-PK and
relationship reads, two filtered/exact roots, partial fields and null, batched
watchers, accepted layers under stale traffic, atomic causal confirmation,
same-entity out-of-order confirmation/rejection, revision/tombstone recreation,
confirmed-only SSR restore, GC, transaction rollback, Apollo's public-API gap,
declaration isolation, and GC safety across destructive and stacked optimistic
layers.
