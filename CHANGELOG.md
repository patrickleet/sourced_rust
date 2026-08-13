### What's changed in v4.0.0

* chore(deps): update actions/checkout action to v7 (by @renovate[bot])

* chore(deps): update actions/upload-artifact action to v7 (by @renovate[bot])

* chore(deps): update marocchino/sticky-pull-request-comment action to v3 (by @renovate[bot])

* chore: atc gitignore (by @patrickleet)

* feat: GraphQL query edge, client replica, and e2e-ui demos (#127) (by @patrickleet)

  BREAKING CHANGE: * feat(e2e-ui): polish demos, event domains, blob URL routes

  Teach-first home and cleaned UI across todos/chat/session/admin/blob.
  Todo/chat use public #[event] command methods; handlers stay thin.
  Blob moves to [[gameId]] with local board paint so New game updates
  immediately while history still uses the shared cache.

  * fix(e2e-ui): wait for move confirms before blob next level

  Optimistic level-complete painted the board before the finishing move
  committed, so start_level loaded an incomplete aggregate and rejected.
  Gate Next level on server-confirmed completion and await the move drain.

  * fix(e2e-ui): restore todo/chat event wire formats

  The #[event]+when= domain polish changed todo.completed/reopened/archived
  payloads (empty → owner_id) and renamed (title → owner_id,title). Existing
  streams failed hydrate, so load/command paths looked like nothing saved.
  Restore Result + record_* command/event pairs with the original payloads;
  handlers match again. Chat restored for the same hard-error pattern.

  * fix(e2e-ui): drop fake board from blob empty state

  Show only copy + Start game when no game is selected, so the empty
  state does not look like a broken half-rendered grid.

  * test(e2e-ui): add Playwright browser e2e suite

  Chromium flows against the live Fieldnote UI: OIDC Login V2 as alice/admin,
  todos lifecycle, chat post, blob start/move, session, unauth redirects, and
  role-gated admin. Run with make up + make run, then make test-browser.

  * ci: run e2e-ui offline suite and Playwright on PR/main

  Add integration-e2e-ui workflow (make test + Docker stack + browser e2e)
  and wire it into on-pr-quality and on-push-main so Fieldnote fixture
  regressions gate merges and releases.

  * fix: green CI for e2e-ui HTTP surface, m2m where, blob start

  Disable public HTTP command wildcards (T0 404) while mounting Zitadel
  ingress/scrape explicitly. Include shadow/through tables in GraphQL
  surface so m2m relationship predicates appear on bool_exp. Survive
  blob /blob → /blob/{id} remount via remembered rows and list-seed merge.

  * fix(e2e-ui): hydrate-safe blob start + align offline UI contracts

  Wait for client hydration before Start/New game, paint from mutation
  payload without remount-racing goto, and assert blob_games_start over
  the network in Playwright. Update api-contract tests for the current
  home story and generated command policies re-export.

  * feat: extract Distributed JS client package

  Implements [[tasks/distributed-js-client-1]]

  * feat: select normalized cache engine

  Implements [[tasks/graphql-qs-client-replica-2]]

  * feat: emit authorized client manifests

  * chore: refresh e2e GraphQL artifacts

  * chore: refresh e2e command artifacts

  * feat: add normalized client replica

  * feat: add typed causal command contracts

  Implements tasks/graphql-qs-client-replica-4.

  * feat: add durable causal command dispatch

  Implements [[tasks/graphql-qs-client-replica-5]]

  * feat: harden causal projection protocol

  Implements tasks/graphql-qs-client-replica-15.

  * feat: expose causal GraphQL client protocol

  * feat: add unified client compiler foundation

  * feat: harden client compiler contracts

  * feat: compile recursive client replica plans

  Implements [[tasks/graphql-qs-client-replica-6]]

  * feat: complete compiler-backed client plans

  * docs: move durable decisions to distributed kb

  * feat: bind clients to authorization surfaces

  Implements the server and compiler slice of [[tasks/graphql-qs-client-replica-10]].

  * fix: keep manifest validation test current

  Covers [[tasks/graphql-qs-client-replica-10]].

  * feat!: complete compiler-backed client replica

  Generate role-scoped query and command artifacts from Rust Surface IR, make the normalized causal replica the only public client runtime, and migrate the SvelteKit e2e consumer to the generated contract.

  BREAKING CHANGE: remove the pre-release document cache, manual target/reconcile APIs, raw GraphQL command catalog, and pilot persistence format.

  * fix: isolate GraphiQL introspection limits

  * fix!: split query authority from causal comparability

  * fix: retain projection poll liveness

  * fix: preserve stale replica views during revalidation

  Implements [[tasks/graphql-qs-client-replica-18]]

  * fix: stabilize optimistic command feedback

  * fix: keep todo command controls visually stable

  * fix: preserve optimistic order while revalidating

  * fix: fence stale revalidation after projected commands

  * fix: retain projected rows until read-model echo

  * feat: add direct-only projection owners

  Implements [[tasks/graphql-qs-client-replica-20]].

  * fix(cli): treat exact --documents paths as literal files (#136)

  Skip glob expansion when a --documents value is an existing file so path
  segments like SvelteKit [[gameId]] are not interpreted as character classes.
  Globs continue to expand as before for patterns that are not existing files.

  * fix: enforce asserted roles in strict OIDC mode

  Implements [[tasks--graphql-qs-epic]]

  * fix: share expiring OIDC JWKS cache per engine

  Implements [[tasks--graphql-qs-epic]]

  * test: cover exact GraphQL document paths

  Implements [[tasks--graphql-qs-epic]]

  * fix: address GraphQL query service review findings

  Implements [[tasks--graphql-qs-epic]]

  * fix: keep projected fences against stale high-revision snapshots

  Conflicting GraphQL snapshot bodies must not override a direct projection
  even when response evidence is stamped later than the command. That race
  was rolling back live e2e blob moves after held revalidation responses.

  * fix: fence projected rows by response start

  Implements [[tasks--graphql-qs-epic]]

  * docs: position Distributed as full-stack CQRS + GraphQL + JS

  Expand the README opening, design goals, GraphQL section, e2e-ui template
  table (blob, OIDC, Projected), and @hops-ops/distributed client surface so the
  docs match the platform scope of the query-service epic.

  * docs: lead README with Fieldnote, Blob, and live demos

  Open with runnable full-stack examples (e2e-ui, GraphiQL, capability map)
  before dependency wiring and the library quick start.

  * docs: highlight first-class OIDC and multi-IdP e2e

  Call out built-in OidcBearer on the GraphQL edge and the live Zitadel,
  Keycloak, and Authentik compose/test suites in the README demos and identity
  section.

  * docs: lead demos with e2e-ui code index

  Point root README at the Fieldnote files that make SSR, live chat,
  Projected blob, dual client surfaces, and OIDC feel product-grade.
  Rewrite tests/e2e-ui/README as the deeper code map plus runbook.

  * docs(e2e-ui): refresh home walkthrough for latest patterns

  Update the Fieldnote index route code tour: Fact vs Projected,
  dual client surfaces, CausalCommandContext, real RLS/effects
  samples, and admin as a separate generated client.

  * refactor(e2e-ui): nest CSS natively; keep postcss for custom-media

  Nest app.css and route/component styles with native nesting (&, &-,
  nested @media). Leave nesting-rules off in postcss-preset-env so
  BEM &-suffix is not mis-expanded; keep custom-media and range queries.

  * revert(e2e-ui): undo CSS nesting rewrite

  Restores flat selectors that postcss/Vite were expanding incorrectly
  (&-suffix → -suffix.parent). Style architecture refactor should be
  component extraction + scoped Svelte CSS, not a global nest pass.

  * refactor(e2e-ui): product components + split global CSS

  Extract scoped product primitives (AppPage, PageHeader, InlineAlert,
  Panel, StatRow) and extend Button with ink/ghost/quiet/sm. Migrate
  todos/chat/blob/admin onto them. Split the 1.6k app.css into tokens
  (app.css), layout chrome (chrome.css), and home-only wireframe
  (home.css) so shared CSS is intentional and route/product UI can use
  Svelte scoping.

  * fix(e2e-ui): polish chat and blob after product shell migration

  Restore chat shell CSS variables and live-status states; tidy blob
  markup under AppPage.

  * fix(e2e-ui): read Zitadel project roles from access token

  Admin grants live on the access token (urn:zitadel:iam:org:project:roles),
  not the id_token. Session only decoded the id_token, so admin always became
  user. Merge groups from both tokens and align session/nav admin checks.

  * fix(e2e-ui): type-cast refreshed token when storing groups

  * fix(e2e-ui): always request Zitadel project role scopes at login

  Admin project role is granted in bootstrap, but tokens only include
  role claims when authorize requests the reserved roles scopes. Merge
  those scopes into every OIDC start (not only a complete OIDC_SCOPES
  env) and read roles from access + id tokens including project-id claims.

  * fix(e2e-ui): let admin use the normal fieldnote app surface

  fieldnote was registered for role user only, so sessions with concrete
  role admin failed protocol selection on todos/chat/blob. Register the
  shared surface for admin+user (admin still uses fieldnote-admin for
  elevated ops) and regenerate clients.

  * refactor: split distributed-replica into folder modules

  Move the monolithic replica implementation into package-private modules
  (types, constants, clocks, hydration, optimistic, helpers, watch, impl)
  while keeping createDistributedReplica on the same public export path.

  * fix(e2e-ui): align Playwright with product shell and admin hydration

  Todo specs still targeted removed fn-* classes after the product panel
  refactor. Update selectors, wait for fieldnote-admin hydration before
  force-archive, assert archive on the clicked row, and match optimistic
  order checks to create/reopen list behavior.

  * refactor: split command-runtime into folder modules

  Move the monolithic command runtime into package-private modules
  (symbols, types, errors, lifecycle, helpers, create) while keeping the
  same public export path for createReplicaCommandRuntime and related APIs.

  * refactor: split cache-engine into folder modules

  Move the private purpose-built cache engine into types, errors, helpers,
  engine, and create modules while preserving the internal import path for
  createCacheEngine, cacheIndexKey, and related types.

  * refactor: split query-plan into filter/order/pagination modules

  Move portable query-plan evaluation into focused modules (types, resolve,
  filter, order, pagination, util) while keeping the same public export
  path for evaluateReplicaFilter, compareReplicaOrder, and pagination.

  * refactor(js): extract shared helpers into src/lib

  Deduplicate deepEqual, reportSafely/reportUnhandled, isPlainRecord,
  compareCodeUnits, assertName, and freezeRecord behind js/src/lib so
  cache-engine, replica, and command-runtime share one implementation.

  * fix(js): clear unused imports so tsc check matches CI

  Module splits left type-only and value imports unused; npm test only
  runs build (no unusedLocals), while CI quality runs check --noEmit and
  failed. Strip unused imports so npm run quality is green.

  * refactor(js): split commands into types, prepare, presets, receipt

  Extract command artifact types, contract errors, and implementation so
  prepare/inventory/receipt concerns are separable. Public exports stay on
  ./commands.js. Implements [[tasks/js-less-context-7]].

  * refactor(js): split identity, helpers barrels, persistence, index, diagnostics

  - identity: keys/codec/clone barrels over implementation
  - command-runtime: concern barrels over helpers-impl
  - persistence, index-maintenance, diagnostics: folder + thin re-exports

  Implements [[tasks/js-less-context-8]] [[tasks/js-less-context-9]]
  [[tasks/js-less-context-10]] [[tasks/js-less-context-11]]
  [[tasks/js-less-context-12]].

  * refactor(js): extract distributed-replica impl concern modules

  Move fetch/live, protocol generation, optimistic layers, dehydrate/hydrate
  orchestration, and diagnostics emission into package-private helpers with
  thin class delegates. Implements [[tasks/js-less-context-2]] through
  [[tasks/js-less-context-6]].

  * refactor(js): body-split commands, identity, helpers, persistence, index, diagnostics

  Move function bodies into concern modules instead of thin re-export barrels so
  agents load only the needed concern. implementation.ts/helpers-impl.ts become
  re-export surfaces. Quality: 253 tests + check + pack:smoke + publint.

  Implements [[tasks/js-less-context-7]] [[tasks/js-less-context-8]]
  [[tasks/js-less-context-9]] [[tasks/js-less-context-10]]
  [[tasks/js-less-context-11]] [[tasks/js-less-context-12]]

  * refactor(js): move command-runtime helpers into lib/

  Bodies live under command-runtime/lib/{inventory,binding,effects,transport,
  status,output,projection,util}.ts. helpers.ts remains the stable barrel;
  helpers-impl.ts re-exports helpers.

  * refactor(js): drop command-runtime helpers.ts barrel

  Barrel lives at lib/index.ts; create.ts imports from ./lib/index.js.
  Removes helpers.ts and helpers-impl.ts naming leftover.

  * refactor: split graphql compile modules

  Implements [[tasks/rust-less-context-6]]

  * refactor: split client compiler manifest module

  Implements [[tasks/rust-less-context-3]]

  * refactor: split client compiler graphql pipeline

  Implements [[tasks/rust-less-context-2]]

  * refactor: split client compiler render module

  Implements [[tasks/rust-less-context-4]]

  * refactor: split client manifest module

  Implements [[tasks/rust-less-context-7]]

  * refactor: split command ledger module

  Implements [[tasks/rust-less-context-12]]

  * refactor: split sqlx repository core

  Implements [[tasks/rust-less-context-13]]

  * refactor: split command manifest compiler

  Implements [[tasks/rust-less-context-5]]

  * refactor: split graphql protocol helpers

  Implements [[tasks/rust-less-context-9]]

  * refactor: split GraphQL command contract modules

  Implements [[tasks/rust-less-context-8]]

  * fix(macros): update trybuild paths after command_contract split

  Compile-fail diagnostics now point at command_contract/effect_wire.rs
  (and sibling modules) instead of the old monolith path.

  * refactor: split projection protocol store module

  Implements [[tasks/rust-less-context-15]]

  * fix: re-export ProjectionPartitionSnapshot from store split

  The type lived in store/query.rs but was omitted from store/mod.rs
  pub(crate) re-exports, breaking graphql/sqlx consumers.

  * refactor: split projection protocol codec

  Implements [[tasks/rust-less-context-16]]

  * refactor: split hot rust modules

  Implements [[tasks/rust-less-context-23]]

  * refactor: split projector runtime module

  Implements [[tasks/rust-less-context-18]]

  * refactor: split sqlx read model module

  Implements [[tasks/rust-less-context-14]]

  * refactor: split distributed macros entry modules

  Implements [[tasks/rust-less-context-22]]

  * refactor: split graphql surface modules

  Implements [[tasks/rust-less-context-10]]

  * refactor: split graphql engine orchestration

  Implements [[tasks/rust-less-context-11]]

  * refactor: split microsvc service module

  Implements [[tasks/rust-less-context-17]]

  * refactor: folderize SQLx projection protocol

  Implements [[tasks/rust-less-context-19]]

  * refactor: folderize in-memory projection protocol

  Implements [[tasks/rust-less-context-20]]

  * refactor: share pure projection backend helpers

  Implements [[tasks/rust-less-context-21]]

  * refactor: share projection kind storage decoding

  Implements [[tasks/rust-dry-2]]

  * refactor: share projection digest helper

  Implements [[tasks/rust-dry-3]]

  * refactor: share projection failure batch predicate

  Implements [[tasks/rust-dry-4]]

  * refactor: share projection ownership validation

  Implements [[tasks/rust-dry-5]]

  * test: share projection protocol scenarios

  Implements [[tasks/rust-dry-6]]

  * refactor!: rename client GraphQL protocol version 2 → 1

  Never-released wire family; first public ship should be protocol v1, not v2.
  Constants, envelopes, compiler emit, client checks, fixtures, and docs only.
  Left unrelated versions alone (variableCodec v2, digest domain tags, package semver).

  * fix: update postgres/cli fingerprint goldens for protocol v1

  All-features and distributed_cli integration asserted stale schema
  fingerprints that include protocol_version in the hash input.

  * refactor!: ship unreleased wire versions as v1

  Normalize client manifest, variableCodec, command extension slots, and
  protocol-manifest epoch to 1 so the first public release does not imply
  prior public wire families. Refresh fingerprints, goldens, and fixtures.

  * fix: update orders harness schema fingerprint for v1 wire versions

  Ignored distributed_cli integration test expected the pre-v1 schema
  fingerprint; regenerate to match manifest/epoch/slot version renames.

  * fix: update postgres role-surface schema fingerprint for v1

  all-features client_surface_parity asserted a pre-v1 schema hash for the
  orders postgres role surface after wire version renames.

  * chore: drop NPM_RELEASE_READY gate from tag publish

  Bootstrap is complete; a permanent repo variable is unnecessary. Preflight
  still validates the tag form and that @hops-ops/distributed exists on npm.

  * fix(e2e-ui): drop dead Manage Account and Protected Page links

  Remove account-menu link to Zitadel console and session page button to
  /protected, which has no route in the template.

  * chore(e2e-ui): refresh generated clients after protocol v1 wire renames

  Schema fingerprints in the checked-in user/admin clients drifted after
  manifest/codec/slot versions shipped as v1. Keep gen-client outputs in
  lockstep for make check-client / CI drift gates.

  * chore: apply cargo fix for unused re-exports

  Drop unused imports/re-exports reported on default cargo build. Does not
  remove underlying types or methods still present for internal/store use.

  * chore: silence false dead_code noise without deleting protocol surface

  Default cargo build warns on helpers whose callers live behind graphql/sqlx
  or unit tests. Drop true unused re-exports/imports, restore feature-gated
  re-exports that cargo fix removed, and allow dead_code only on intentional
  store/protocol surface and test-oriented wrappers so product builds stay clean.

  * fix: restore test-only re-exports removed by cargo fix

  cargo fix / dead_code cleanup dropped re-exports that unit tests import
  (client_manifest_from_surface, ProjectionCheckpointProbe, obligation
  resolution types). Non-test lib builds still allow unused_imports so
  featureless cargo build stays quiet; lib tests compile again.

  * refactor(e2e-ui): rename Fieldnote demo to Todos

  Drop the invented product name. Surface IDs are todos / todos-admin so the
  demo is obviously a todos app for engineers, not a fictional brand.

  * refactor(e2e-ui): name template e2e-ui; keep Todos as one demo

  Surfaces are e2e-ui / e2e-ui-admin. Todos is only the /todos demo (alongside
  chat, blob, admin)—not the brand for the whole fixture.

  * feat: infer natural read-model storage names

  Implements [[tasks/graphql-qs-projection-model-5]]

  * refactor: rename successful command outcomes

  Implements [[tasks/graphql-qs-projection-model-2]]

  * feat(js): replace optimistic layers atomically

  Evaluate corrections against the target prefix, preserve layer metadata, and publish only the final rebased graph. Add typed missing-layer handling and private replica coverage.

  Implements [[tasks/graphql-qs-projection-model-14]]

  * feat: add typed domain-event occurrence runtime

  Implements [[tasks/graphql-qs-projection-model-3]]

  * feat: generate sourced domain-event capture

  Implements [[tasks/graphql-qs-projection-model-4]]

  * feat: define portable projection program IR

  Implements [[tasks/graphql-qs-projection-model-7]]

  * Implement fluent projection commit API

  Implements [[tasks/graphql-qs-projection-model-6]]

  * Implements [[tasks/graphql-qs-projection-model-8]]

  * Fix projection direct candidate classification

  * feat: add projection catalog identities and placement

  Implements [[tasks/graphql-qs-projection-model-9]]

  * test: align compile failures with succeeded outcomes

  * Implement causal projection executor and graph workspace

  Implements [[tasks/graphql-qs-projection-model-10]]

  * feat: declare exact command projection events

  * feat: bind modeled projections to GraphQL surfaces

  * feat: expose crate projection expression views

  * fix: authorize modeled projection surfaces

  * feat: type sourced domain event contracts

  * fix: seal adapter event contracts

  * fix: witness exact domain event bodies

  * test: refresh event contract diagnostics

  * feat: export strict authorized projection manifests

  * feat: model authorized optimistic projection occurrences

  * feat: define role-safe projection delta v1

  * chore: export projection delta module

  * fix: seal projection delta authority

  * fix: canonicalize projection delta final state

  * feat: lower sealed role-safe projection deltas

  * fix: harden projection delta recovery scopes

  * feat: expose role-safe projection partitions [[tasks/graphql-qs-projection-model-13]]

  * test: freeze projection delta v1 vectors

  Adds the cross-language canonical fixture, exact boundary coverage, authorization transition matrices, placement and provenance guards, relationship/delete cases, and manifest slot evidence.

  Implements [[tasks/graphql-qs-projection-model-13]]

  * feat: parse projection manifest v2 contracts

  Implements [[tasks/graphql-qs-projection-model-15]]

  * feat: compile projection preview artifacts

  Ports ProjectionDelta wire-v1 validation and canonical ordering, lowers command projection previews with conservative recovery, and emits fail-closed command artifact v2 without legacy effects or confirmations.

  [[tasks/graphql-qs-projection-model-15]]

  * test: lock projection compiler contracts

  Completes compiler validation, conservative relationship recovery, artifact v2 fixtures, and cross-layer regression coverage for projection programs, bindings, previews, and the frozen ProjectionDelta wire contract.

  [[tasks/graphql-qs-projection-model-15]]

  * fix: prove optimistic projection inputs

  Requires complete value-bearing upserts, preserves keyed relationship invalidation authority, and validates preview provenance against consistent opaque slot types with fail-closed numeric and shape semantics.

  [[tasks/graphql-qs-projection-model-15]]

  * fix: close projection preview safety gaps

  Separates absent values from clearing intent, proves scalar record keys, bounds expansion before allocation, narrows invalidation and revalidation scope, and emits an explicit compiler-owned artifact-v2 seam for Task 16.

  [[tasks/graphql-qs-projection-model-15]]

  * fix: fail closed at projection runtime seam

  Matches frozen composite-expression absence semantics, treats nested unset as non-destructive uncertainty, and omits artifact-v2 dispatch through the v1 JavaScript runtime while retaining typed inspectable artifacts.

  [[tasks/graphql-qs-projection-model-15]]

  * fix: fail closed on first-present unset

  Preserves frozen FirstPresent semantics by skipping only absent values, treating nested unset as non-destructive uncertainty, and proving record-scoped recovery without optimistic writes.

  [[tasks/graphql-qs-projection-model-15]]

  * Add authenticated projection metadata authority [[tasks/graphql-qs-projection-model-11]]

  * Persist modeled projection receipts exactly [[tasks/graphql-qs-projection-model-11]]

  * Mount modeled projectors and derive causal obligations [[tasks/graphql-qs-projection-model-11]]

  * Format Task 11 runtime changes [[tasks/graphql-qs-projection-model-11]]

  * Bound modeled replay metadata lifetime and decoding [[tasks/graphql-qs-projection-model-11]]

  * Format integrated projection runtime [[tasks/graphql-qs-projection-model-11]]

  * feat: adapt modeled direct projections to exact proof [[tasks/graphql-qs-projection-model-17]]

  * Bind modeled direct proof to active program [[tasks/graphql-qs-projection-model-17]]

  * feat(projections): implement atomic snapshot adapters [[tasks/graphql-qs-projection-model-12]]

  * Harden projection conformance proofs [[tasks/graphql-qs-projection-model-12]]

  * feat: migrate Todo and Chat projection leaves [[tasks/graphql-qs-projection-model-18]]

  * Keep purge descriptor test-only [[tasks/graphql-qs-projection-model-18]]

  * Fix FK-authoritative relationship delegation [[tasks/graphql-qs-projection-model-12]]

  * fix: keep purged todos terminal [[tasks/graphql-qs-projection-model-18]]

  * Test hydrated purge terminality [[tasks/graphql-qs-projection-model-18]]

  * Reject mixed direct projection epochs [[tasks/graphql-qs-projection-model-17]]

  * Harden direct projection owner compatibility [[tasks/graphql-qs-projection-model-17]]

  * feat: apply authoritative projection deltas in JS [[tasks/graphql-qs-projection-model-16]]

  * fix: harden JS projection reconciliation [[tasks/graphql-qs-projection-model-16]]

  * fix: close projection reconciliation audit gaps [[tasks/graphql-qs-projection-model-16]]

  * fix: close projection runtime follow-up gaps [[tasks/graphql-qs-projection-model-16]]

  * feat: model Blob direct projection [[tasks/graphql-qs-projection-model-19]]

  * Implement modeled projection developer experience [[tasks/graphql-qs-projection-model-20]]

  * Fix modeled topology integration [[tasks/graphql-qs-projection-model-20]]

  * Harden modeled projection rollout integration [[tasks/graphql-qs-projection-model-20]]

  * Fence draining projection replay with lifecycle proofs [[tasks/graphql-qs-projection-model-20]]

  * Revalidate terminal projection receipt replays [[tasks/graphql-qs-projection-model-20]]

  * Fix projection draining revalidation bridge [[tasks/graphql-qs-projection-model-20]]

  * Bind empty projection receipts to command contracts [[tasks/graphql-qs-projection-model-20]]

  * Classify mixed draining projection fallback [[tasks/graphql-qs-projection-model-20]]

  * Correct deletion projection examples [[tasks/graphql-qs-projection-model-20]]

  * Fix generated projection UI integration [[tasks/graphql-qs-projection-model-20]]

  * Migrate typed command manifest coverage [[tasks/graphql-qs-projection-model-20]]

  * Strengthen projection manifest coverage [[tasks/graphql-qs-projection-model-20]]

  * Accept embedded model invalidation [[tasks/graphql-qs-projection-model-20]]

  * Constrain embedded invalidation authority [[tasks/graphql-qs-projection-model-20]]

  * Fix PR 127 CI regressions [[incidents/pr-127-ci-failures-after-projection-model]]

  * Wait for durable Todo commands in live test [[incidents/pr-127-ci-failures-after-projection-model]]

  * Fix application projection visibility authority [[incidents/pr-127-ci-failures-after-projection-model]]

  * Infer projected responses from read models [[tasks/graphql-qs-projection-model-21]]

  * Restore explicit CQRS fixture boundaries [[tasks/graphql-qs-projection-model-22]]

  * chore: better version in e2e-ui of test

  * feat: add event-independent mutation IR and dual-path projectors

  Introduce the public mutation authoring path from the domain-event-projections
  spec: versioned MutationProgram IR, mutation! macro, ReadModel capabilities
  metadata, portable handler catalog, server/cache/preview interpreters, and
  shared Rust/JS golden vectors. Wire SAVE_TODO/DELETE_TODO, SAVE_CHAT_MESSAGE,
  and SAVE_BLOB_GAME mutations into e2e fixtures while retaining projection!
  as the dual-path runtime mount until full cutover.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]
  Implements [[tasks/graphql-qs-mutation-projectors-2]]
  Implements [[tasks/graphql-qs-mutation-projectors-3]]
  Implements [[tasks/graphql-qs-mutation-projectors-4]]
  Implements [[tasks/graphql-qs-mutation-projectors-5]]
  Implements [[tasks/graphql-qs-mutation-projectors-6]]
  Implements [[tasks/graphql-qs-mutation-projectors-7]]
  Implements [[tasks/graphql-qs-mutation-projectors-8]]
  Implements [[tasks/graphql-qs-mutation-projectors-9]]
  Implements [[tasks/graphql-qs-mutation-projectors-10]]

  * feat: placement-selected projected() and wire mutation programs on service path

  - Add placement-selected direct registry so commands call commit()?.projected()
    without naming a projection selector (Blob handlers updated).
  - Register BlobGames executor at e2e service construction.
  - Drive SAVE_TODO/DELETE_TODO/SAVE_CHAT_MESSAGE/SAVE_BLOB_GAME mutation
    programs from service registration and event handlers (not test-only stubs).
  - Hide event-owning projection! authoring from public docs; document
    .project(...) as non-preferred for application commands.
  - Unit proof: placement_selected_projected_uses_registered_executor_without_project_selector

  Implements [[tasks/graphql-qs-mutation-projectors-1]]
  Implements [[tasks/graphql-qs-mutation-projectors-10]]
  Implements [[tasks/graphql-qs-mutation-projectors-11]]

  * feat: drive e2e mounts from mutation IR rewrite (real resolve path)

  Replace event-owning projection! for TODO/CHAT/BLOB e2e mounts with
  mutation-backed ProjectionDescriptor factories:

  - program/resolve built via program_from_mutation_arms (SAVE_*/DELETE_*)
  - lower via shared lower_single_model ORM path
  - service construction asserts descriptor program bytes == mutation rewrite
  - handlers apply those descriptors without id()-only theater
  - Blob eligibility fixtures still use projection! for compile_fail only

  Also expose ResolvedProjectionPlan::resolve publicly for dual-path factories.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * feat: WP10 cutover — remove projection! and command-side .project

  Hard-delete competing projector authoring surfaces:

  - Remove event-owning `projection!` proc-macro export and source module
  - Delete distributed_macros projection unit/compile_fail suites
  - Remove CausalCommitBuilder/CausalRepository `.project(...)` selectors
  - Drop crate-root `ReadModelWritePlanBuilder` re-export (adapter remains at
    `distributed::read_model::ReadModelWritePlanBuilder` and TableWritePlan)
  - Migrate residual framework/test descriptors to mutation-backed factories
  - Add tests/legacy_authoring_absence.rs structural gate

  Task-11 matrix (fmt, clippy -D, workspace tests, npm quality, e2e-ui
  check-client/test) green at this cutover; browser/Postgres live env gates
  remain environment-dependent.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]
  Implements [[tasks/graphql-qs-mutation-projectors-11]]

  * feat: AC4 residual cutover — remove effects macros and public projector ORM export

  Hard-delete separately authored command_effects!/command_confirmations! and
  TypedCommand::{effects,confirmations}; demote ProjectionReadModelWorkspace to
  crate-private; docs teach mutation! + .emits/.preview; expand structural gate;
  restore command_input_defaults trybuild suite under a dedicated name.

  Full task-11 matrix green (fmt, clippy -D, cargo test workspace all-features,
  npm quality, e2e-ui offline, legacy_authoring_absence).

  Implements [[tasks/graphql-qs-mutation-projectors-1]]
  Implements [[tasks/graphql-qs-mutation-projectors-11]]

  * docs: teach placement-selected Blob projected path only

  Replace residual repo.project(BLOB_GAMES).commit(...).projected() examples
  in e2e-ui README and walkthrough with the live
  repo.commit(game)?.projected() path. Keep compile_fail/migration notes only
  for the removed selector.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]
  Implements [[tasks/graphql-qs-mutation-projectors-11]]

  * refactor: release-ready cleanup of leftover projector authoring surfaces

  Delete the crate-private graph_workspace ORM path, strip dead command_effects
  authoring types/re-exports and constructors, rename macros module to
  command_input_defaults, and tighten structural gates. Keep protocol lifecycle
  primitives on CausalProjectorContext and low-level TableWritePlan adapters.

  Full matrix green (fmt, clippy -D, cargo test workspace, npm quality, e2e-ui).

  Implements [[tasks/graphql-qs-mutation-projectors-1]]
  Implements [[tasks/graphql-qs-mutation-projectors-11]]

  * feat: mutation_projector! sugar and arm helpers for nice app mounts

  Framework owns resolve/lower/inventory factory glue via mutation_projector!
  and arm_state_upsert_for_model / arms_state_upsert_for_model /
  arm_delete_pk_from_envelope / build_mutation_projector_program.

  e2e todos/chat/blob projections shrink to mutations + event arms + mount.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * docs(e2e-ui): show mutation_projector! author surface in README

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * feat: portable_handlers! matches the spec author model

  App authors declare mutations and which events apply them. Dual-path
  projection IR compile stays internal (compile_portable_handlers).

  Rename public bind_* helpers; hide arm_* vocabulary from the crate root
  docs. Rewrite e2e todos/chat/blob to portable_handlers! / bind language.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * fix: import DomainEventContract in todo projection tests

  * refactor: remove dual-path compatibility surface; ship portable_handlers only

  Delete arm_* / build_mutation_projector_program shims and MutationProjectionArm.
  Public API is mutations + bind_* + portable_handlers! / compile_portable_handlers.
  Rename opaque compile unit to PortableHandler. Strip e2e program wrappers and
  service dual-path equality asserts.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * refactor(e2e): rename TODO_READS to TODOS for model-consistent naming

  Matches BLOB_GAMES and CHAT_MESSAGES — the mount is the Todos model
  handlers, not a "reads" collection.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * refactor(e2e-ui): consistent command previews and portable_handlers demos

  Inline todo.complete preview with the other commands via state_preview! on
  the typed command. Remove complete_preview from projections. Align event
  handler comments and UI walkthrough with mutation + portable_handlers.

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * feat: event-first mutations, applies mapping, handler-owned projected commits

  Ship the revised domain-event projection authoring model for the mutation
  projectors cutover:

  - GraphQL-looking mutation documents via mutation_file! / mutation! (syntax-only IR)
  - Event-first portable_handlers! (on <events> apply <mutation>)
  - Command .applies for known mutation-input mapping (preview kept as alias)
  - Blob projected path: Mutation::from_state + readmodel(row).commit()?.projected()
  - e2e command modules renamed with aggregate prefix (todo_reopen, …)

  Implements [[tasks/graphql-qs-mutation-projectors-1]]

  * fix: keep chat live+optimism with author joins and portable todo lists

  Live queries that join unowned tables (e.g. chat_messages.author → auth_users)
  no longer force live.supported=false; index plans skip zero-projector join
  tables so the WebSocket can stay active. Nullable missing relationship edges
  stay complete so optimistic rows remain materializable, and the chat UI no
  longer blanks the list while incomplete.

  Also: application surface uses user grants only so Todos/BlobGames keep a
  portable owner row policy for optimistic list inserts; chat unit partition for
  lobby resume; iMessage-style Sent/Delivered footers and AuthUsers display
  names on chat.

  Implements follow-ups for [[tasks/graphql-qs-mutation-projectors-1]]

  * fix: keep warm same-scope cache and polish lobby chat history

  Same-scope soft-nav rehydrate merges route SSR seeds instead of wiping
  confirmed records/indexes omitted from the page. Lobby chat uses a fixed
  scroll panel, page size 25, infinite history, and Chromium-correct
  column-reverse scroll geometry (negative scrollTop).

  * fix: restore quality and e2e-ui offline CI on mutation-projectors branch

  Gate causal_direct_v1_program behind the graphql feature so default/sqlite
  lib tests compile, and send the exact e2e-ui application surface roles list
  expected by select_protocol_surface.

  * fix(e2e-ui): grant admin principals the user role too

  Local Zitadel bootstrap and offline suite tokens assert both admin and
  user for admin humans/machines so they can use normal app surfaces and
  elevated paths without changing model permissions.

  * fix(e2e-ui): write create_human status to stderr for clean UIDs

  Command-substitution of create_human was capturing "reusing human …"
  lines into E2E_HUMAN_*_UID. Keep only the user id on stdout.

  * fix: multi-role application surfaces + chat history load race

  Make multi-role principals first-class for named application surfaces:
  eligible roles control protocol open (any asserted role may open), while
  schema privilege roles shape the portable client contract. e2e-ui keeps
  eligible {admin,user} with schema {user} so owner-portable optimism is
  preserved without collapsing model permissions.

  Also stop treating incomplete/empty history pages as end-of-history while
  the live chat window is still filling, and harden the Playwright history
  scroll test under column-reverse.

  Implements multi-role surface selection for [[tasks/graphql-qs-mutation-projectors-1]]

  * fix(e2e-ui): update surface structural gate and chat optimism window

  Point the fixture source gate at surface_for_application_contract and
  eligible {admin,user} schema {user}. Widen the chat revalidation
  optimism paint window so CI headroom stays under the delayed mutation
  response without weakening the stale-while-revalidate assertion.

  * feat: set-only identity and surface-privilege GraphQL execution

  Breaking major-release cutover: Session carries x-roles only (no
  priority-picked primary x-role). GraphQL execute/stream binds to the opened
  application surface privilege pack (or a membership-checked role surface).
  Multi-role principals without a named surface fail closed. Anonymous
  eligible surfaces open with empty identity.

  e2e-ui: public e2e-ui-public surface, dual-role admin suite uses admin
  surface for elevated ops, causal grants check any asserted role.

  Implements [[tasks/graphql-qs-surface-authz-1]]

  * fix: prove e2e-ui-public anonymous open and finish AuthZ docs

  Add service test that opens e2e-ui-public with an empty Session and queries
  chat_messages, plus unauthenticated /public route documenting the bare
  protocol path. Specs no longer teach Session::role()/x-role as execution.

  Implements [[tasks/graphql-qs-surface-authz-1]]

  * fix: CI failures for set-only identity (x-roles)

  - graphql_query_protocol: use role binding for x-roles header; WS
    connection_init sends x-roles (not x-role)
  - graphql_oidc_common E1: assert Session::roles() instead of role(),
    which is None under set-only claim mapping

  Implements [[tasks/graphql-qs-surface-authz-1]]

  * fix: remove x-role identity bridges — set-only cutover

  Major-release identity is x-roles only; drop migration paths that still
  accepted or re-injected a singleton primary role.

  - ROLE_KEY is x-roles; Session::roles/has_role are the identity API
  - remove causal ensure_causal_grant legacy x-role fallback
  - schema/metrics privilege fallback uses roles set only
  - OIDC e2e layer re-injects x-roles (not x-role + default user)
  - JS DevHeaders + tests send x-roles
  - keep stripping client x-role as defense-in-depth only

  Implements [[tasks/graphql-qs-surface-authz-1]]

  * fix: align subscription unknown-role test with set-only authority

  Unconfigured singleton roles fail closed at execution-authority resolve
  (same generic surface message), not via legacy primary-role schema lookup.

  Implements [[tasks/graphql-qs-surface-authz-1]]

  * fix(e2e-ui): treat scrape outbox duplicates as skips

  Re-scrape of unchanged profiles hits the content-addressed outbox unique
  key by design. Classify DuplicateOutboxMessageInBatch / unique-violation
  wording as skipped, not errors, so start scrape reports stay clean.

  Implements [[tasks/graphql-qs-surface-authz-1]]

  * feat(e2e-ui): framework home + How it’s built slide-out

  Reframe the home page around Distributed principles and link demos as
  destinations. Each demo route gets a right-hand drawer with tabbed
  walkthroughs (domain → command → projection → client) and a principle
  callout per tab — a Distributed lens on the “code tabs next to the app”
  teaching pattern.

  Implements teaching UX for the e2e-ui template.

  * fix(e2e-ui): hero highlights full-stack CQRS, TS, OIDC, SvelteKit

  Lead the home hero with the end-to-end story: event-sourced CQRS, TypeScript
  clients, first-class OIDC, and SvelteKit SSR/live — not CQRS alone.

  * fix(e2e-ui): wider How-it’s-built panel, no closed shadow, code colors

  Drawer only shadows when open, width ~46rem, and lightweight syntax tint
  for walkthrough samples (keywords, strings, types, attrs, comments).

  * feat(e2e-ui): browser-first How-it’s-built tab order

  Reorder every demo walkthrough: (1) query/live (2) commands + client
  cache optimism vs Projected atomic (3) handlers/repo (4) domain macros
  (5) domain events + projections. Match the teaching path from UI inward.

  * feat(e2e-ui): open lobby chat for anonymous GraphQL reads

  Allow /chat without a session so the anonymous privilege pack is visible
  in the UI: e2e-ui-public client for guests, sign-in CTA instead of the
  composer, require_auth=false for empty OIDC identity, and AuthUsers read
  for public author joins.

  * feat(e2e-ui): show RBAC on How-it’s-built query and command tabs

  Each demo walkthrough now includes short ModelPermissions / command.roles
  samples on tabs 1 and 2 so read grants and mutation roles sit next to the
  browser query and command story.

  * fix(e2e-ui): replace comment-only How-it’s-built samples with real code

  Walkthrough panels now paste actual handler, domain, projection, RBAC, and
  generated-client snippets from the fixture instead of comment stubs.

  * feat(e2e-ui): rewrite home as Distributed product landing

  Frame the site as the framework homepage — full-stack CQRS pitch, pillars,
  domain→service→client flow, compact playground cards, and local run.
  Demos stay destinations; ops/hosting only noted as roadmap.

  * style(e2e-ui): product-home layout styles for dist-* sections

  * fix(e2e-ui): reframe home as GraphQL realtime → owned write model

  Drop Meteor comparisons. Pitch Distributed as the next step after live
  GraphQL/query engines: keep the realtime client feel, add event-sourced
  commands, projections, OIDC surfaces, and generated TS clients.

  * fix(e2e-ui): home is CQRS/domain-first; GraphQL is transport

  Position Distributed around event-sourced commands, projections, and an
  honest client replica. GraphQL and SvelteKit are how the playground speaks,
  not the product definition.

  * fix(e2e-ui): brand Distributed; hero product definition

  Rename header/footer e2e-ui → distributed. Hero states Distributed as a
  cloud-native Rust and TypeScript framework for simple realtime, performant,
  scalable apps on distributed-systems foundations.

  * fix(e2e-ui): hero — start simple, scale to microservices

  * feat(e2e-ui): product home Features from CQRS/ES canon narrative

  Rebuild home Features as the owner product story (two models, event-sourced
  aggregates, SQL+RBAC, inferred query edge, projections, browser replica,
  SvelteKit @load/@live, OIDC) with playground code samples and syntax
  highlighting matching How-it's-built panels.

  * refactor: rename portable_handlers! to projection!

  Public event→mutation authoring is now projection! (and compile_projection /
  ProjectionHandler). Matches CQRS product language and ProjectionDescriptor.

  Call sites use distributed::projection! so the macro does not clash with the
  projection module path. Legacy absence tests now assert the declarative macro
  is public while the old event-owning proc-macro stays gone.

  * feat: event-first projection! on { events, mutation, input }

  Replace apply/as and on_deleted with multi-arm on blocks that bind event
  body or aggregate_id into mutation inputs. Align todos/chat/blob, home
  Features (mutation IR + lifetime highlighting), and demos with the new
  surface. Drop mutation_projector and on_state authoring.

  * refactor(e2e-ui): name replica handles query, data by resource

  Prefer query = Op.use() over list, and todos/games/pageMessages over
  generic rows, in app pages, home samples, and How-it's-built demos.

  * feat(e2e-ui): SOTA home story — claim arc, general bar, delivery

  Expand the product home with a Brunson-style arc: claim band, backend/Rust/frontend
  SOTA as general industry bar (not product hooks), handoff to Distributed, then
  backstory and how it delivers. Teach unidirectional + event-driven as one path;
  tighten CAP wording; claim/band styles on home.css.

  * feat(e2e-ui): walkthrough read models + unidirectional flow diagram

  Expand How-it's-built overlays with ReadModel structs, aggregate shapes,
  domain event samples, and projection GraphQL mutations. Replace the home
  system-flow monospace list with a full-width circular dotted diagram.

  * fix(e2e-ui): gate todos/blob on login; drop redundant /public page

  Show Todos and Blob in nav for guests; requireAuth on page loads so
  client-side navigation redirects to /login?callbackUrl=… and returns after
  sign-in. Remove the standalone /public demo (lobby chat already covers
  anonymous). Honor callbackUrl on login/signup when already signed in;
  hero copy mentions realtime applications.

  * feat: Eventual/Atomic command semantics + restore chat optimism

  Ship one mutation IR with two proofs: Eventual (async projector +
  delta/expects) and Atomic (handler row + records). Rename wire/protocol
  states and APIs from causal/projected to eventual/atomic with no aliases.

  Direct placements export .applies previews for client optimism while
  still sealing from the atomic response. Command ledger migrations use
  atomic state (0004 + CHECK updates).

  Fix chat list optimism regressions: belongs_to joins are GraphQL/client
  nullable so missing author edges materialize, and full first-page offset
  indexes accept local optimistic inserts (re-sort + truncate).

  Regenerate e2e-ui clients, demos/docs, and JS tests for the new contract.

  * test(e2e-ui): gate demo optimism offline and in the browser

  Add a shared hold-mutation helper and optimism.user.spec that requires
  chat post (including full first page), todos create/complete, and blob
  move continuity to paint before a delayed GraphQL response.

  Offline optimism-artifacts.test.mjs locks preview IR, atomic
  directProjection, nullable ChatMessages.author, and local first-page
  insert policy so gen/compiler regressions fail without a browser.

  Todos create under the delayed-route order test now asserts list paint
  before the wire returns.

  * fix(e2e-ui): blob move board optimism via .applies input fields

  Wire blob.move like todos/chat: command input carries the optimistic
  board outcome (map_json, score, status, …) and state_preview maps those
  fields into the client optimistic layer. A pure TypeScript twin of
  blob_domain::simulate_move fills the input; the handler still recomputes
  authority from game_id + direction only.

  Regenerate clients (full upsert preview), unit-test simulate_move parity,
  and require paint-before-wire on the player cell in optimism.user.spec.

  * docs(e2e-ui): clarify blob move uses shared .applies optimism path

  * refactor!: drop TypedCommand.preview alias — use applies only

  No back-compat renames. Call sites and docs use .applies. Wire
  vocabulary test expects eventual/atomic only (rejects causal/projected).

  * docs: application composition — logical mounts, process roles, runtime

  Capture the accepted DX direction: same packages re-cut as monolith or
  microservices; Eventual projectors may split; Atomic seals stay collocated.
  Runtime pairs persistence, locks, and bus; process role selects outbox,
  consumer, and GraphQL. Implementation order and e2e-ui collapse targets
  included. Linked from usage skill and e2e-ui README.

  * fix(ci): finish eventual/atomic rename in CLI fixtures and suite asserts

  Update generated-commands fixture, dctl client_compiler/cli_manifest
  expectations, GraphQL protocol tests, and e2e-ui behavioral suite to the
  eventual/atomic wire vocabulary (no causal/projected aliases).

  * fix: repair PR 170 migration and contract drift

  Preserve applied migration history, register migration 4, complete the Eventual/Atomic rename, refresh generated contracts, and repair the affected e2e behavior.

  Resolves [[pr-170-eventual-atomic-rename-and-migration-failur]]

  * test(e2e-ui): consolidate duplicate coverage

  Fold optimistic assertions into product journeys, remove redundant offline and browser scenarios, and retain the unique stale-response race.\n\nImplements [[tasks/e2e-ui-test-maintenance-3]].\nImplements [[tasks/e2e-ui-test-maintenance-4]].

  * test(js): centralize command protocol fixtures

  Route valid command metadata through one canonical fixture while keeping malformed protocol cases explicit.\n\nImplements [[tasks/e2e-ui-test-maintenance-5]].

  * ci: remove duplicate feedback work

  Cancel superseded PR runs, deduplicate e2e-ui setup, narrow compatibility gates, and compile Postgres tests with only required features.\n\nImplements [[tasks/e2e-ui-test-maintenance-6]].\nImplements [[tasks/e2e-ui-test-maintenance-7]].

  * ci: overlap independent e2e-ui offline gates

  Install shared UI prerequisites once, then run the Rust and sequential UI pipelines concurrently.\n\nImplements [[tasks/e2e-ui-test-maintenance-6]].

* feat: land application lifecycle and workbench stack (by @patrickleet)

  BREAKING CHANGE: Squash the complete stacked change set through PR #178 after GitHub's
  stack merge operation landed only the bottom layer.

  Includes PRs #172, #173, #174, #177, and #178.

  BREAKING CHANGE: the standalone CLI binary is named distributed instead
  of dctl, and application surface roles use the eligible/schema split.


See full diff: [v3.3.4...v4.0.0](https://github.com/hops-ops/distributed/compare/v3.3.4...v4.0.0)
