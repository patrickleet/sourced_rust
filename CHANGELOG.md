### What's changed in v4.10.0

* feat: add GraphQL islands for SvelteKit data ownership (#220) (by @patrickleet)

  ##### Why

  `@load` and `@live` were compiled as route-level concerns. That worked for page documents, but layouts and reusable components had to move their queries upward or duplicate route matching, variable extraction, prefetch, SSR, hydration, and live-subscription lifecycle code in the application.

  This change makes a GraphQL document the owner of a portable data island. The compiler describes the island; the SvelteKit adapter decides where it can safely execute.

  ##### What it does

  - Compiles every `@load` / `@live` document into framework-neutral island metadata.
  - Discovers `+page.graphql`, `+layout.graphql`, and GraphQL files colocated with statically imported Svelte components.
  - Promotes component islands to their nearest provable page or layout boundary.
  - Generates one typed boundary plan shared by SSR, hover prefetch, navigation, hydration, component reads, and live retention.
  - Binds variables once from route params, search params, trusted session values, constants, or forwarded props.
  - Keeps layout islands alive across child navigation and releases page/live work at the owning boundary.
  - Deduplicates identical operations retained by more than one boundary.
  - Rejects dynamic imports, cycles, ambiguous ownership, cross-surface placement, unsupported inventory versions, and unbounded layout `@live` work with stable diagnostics.
  - Removes the generated route registry and application-owned operation/variable switches.

  ##### Example

  A reusable component can own its query beside the component:

  ```graphql
  #### src/lib/components/blob/SelectedBlobGame.graphql
  query SelectedBlobGame($gameId: String) @load {
    blob_games(where: { game_id: { _eq: $gameId } }, limit: 1) {
      game_id
      score
      status
    }
  }
  ```

  ```svelte
  <script lang="ts">
    import { SelectedBlobGame } from '$distributed';

    const query = SelectedBlobGame.use({ gameId });
    const selected = $derived($query.data.blob_games?.[0]);
  </script>
  ```

  The application includes both route and component documents:

  ```js
  documents: [
    'src/routes/**/*.graphql',
    'src/lib/components/**/*.graphql'
  ]
  ```

  The root integration consumes the generated boundary inventory:

  ```ts
  const distributed = createDistributedSvelteKitServer({
    boundaries: DISTRIBUTED_BOUNDARY_OPERATIONS,
    getSession,
    getRole,
    getUrl
  });

  const client = provideDistributed({
    boundaries: DISTRIBUTED_BOUNDARY_OPERATIONS,
    browser,
    session
  });
  ```

  Navigation no longer needs an operation-name switch or a second variable mapper:

  ```ts
  client.retainLocation(location, context);
  await client.prefetchLocation(target.pathname, context);
  ```

  A `+layout.graphql` island is retained while navigating among that layout's children, so shared SSR/live data is not torn down and restarted on every page.

  ##### Benefits

  - Components and layouts can own the smallest useful query without coupling their UI to backend transport code.
  - SSR, client navigation, and live updates use the same generated variable contract.
  - Applications stop maintaining parallel route registries and variable extraction logic.
  - Static analysis fails closed when ownership or variable provenance cannot be proven.
  - Island metadata stays framework-neutral; SvelteKit placement is an adapter concern rather than compiler policy.
  - Trust surfaces remain separate through generated per-surface boundary inventories.

  ##### Breaking change

  This is an intentional pre-release cutover:

  - `DISTRIBUTED_ROUTE_OPERATIONS` is replaced by `DISTRIBUTED_BOUNDARY_OPERATIONS`.
  - `createDistributedSvelteKitServer({ routes })` becomes `createDistributedSvelteKitServer({ boundaries })`.
  - `provideDistributed()` receives the boundary inventory.
  - Application route/operation variable switches should be removed in favor of `retainLocation()` and `prefetchLocation()`.
  - Generated `routes.ts` files are replaced by `islands.json`, `islands.ts`, and `boundaries.ts`.

  Regenerate clients and update the root SvelteKit adapter in one coherent build; old route artifacts are not retained.

  ##### Verification

  Passed locally at `3efee138`:

  - `cargo test -p distributed_cli` — 225 unit tests plus package integration/doc tests.
  - `cargo test -p distributed_cli --test cli_client` — 11/11.
  - `node --experimental-strip-types --test tests/generated-client.test.mjs` — 3/3.

  The JavaScript quality and browser suites require a provisioned dependency/full-stack checkout and are delegated to PR CI. The local worktree intentionally did not install dependencies or take over an already-running development stack's ports.


See full diff: [v4.9.0...v4.10.0](https://github.com/hops-ops/distributed/compare/v4.9.0...v4.10.0)
