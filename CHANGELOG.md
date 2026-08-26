### What's changed in v4.3.0

* feat: aggregate roots in celld via WASM (#205) (by @patrickleet)

  * fix: do not await immediate outbox publish on command completion

  Command completion is the durable commit of events and outbox rows.
  Immediate publish still runs after commit, but on a spawned task so the
  caller is not blocked on the bus ack. Publish failure still does not fail
  the command; drain recovers claimed rows at lease expiry.

  Implements [[tasks/outbox-immediate-nonblocking-1]]
  TRANSPORT-REQ-001 / TRANSPORT-GAP-001 [[specs/framework/transports]]

  * test: wait for spawned immediate outbox publish in sqlite tests

  Command completion returns at durable commit; Published is settled by
  the spawned hook. Match the unit-test yield-wait so all-features CI
  does not race.

  Implements [[tasks/outbox-immediate-nonblocking-1]]

  * fix: spawn immediate outbox publish only on a current runtime

  tokio::spawn panics without a runtime, which would fire after a durable
  commit. Spawn through Handle::try_current when one exists; otherwise
  publish inline. Test gate uses notify_one so a late waiter still sees
  the permit.

  Implements [[tasks/outbox-immediate-nonblocking-1]]

  * fix: publish outbox through a bounded worker, not spawn-per-commit

  Commit writes pending rows and try_sends ids onto one drain loop.
  Overflow wakes dispatch_batch. with_bus starts that worker. Hook spawn
  remains the mailbox-less fallback.

  Implements [[tasks/outbox-immediate-nonblocking-1]]

  * feat: mount e2e-ui Todo commands from todo-domain

  SOA Routes::mount installs domain-owned Todo declarations. Service
  module keeps only mounts plus the projector.

  Implements [[tasks/portable-command-hosts-2]]

  * feat: mount e2e-ui Chat and Blob commands from domain crates

  chat.post stays a full handle with created_at policy. Blob Atomic
  commands shard by game_id. Client blobSimulateMove wasm is unchanged.

  Implements [[tasks/portable-command-hosts-3]]

  * fix: harden live Playwright login, admin empty, and chat seed

  Prefer outbox id hints over sleep(0) dispatch_batch so Eventual
  `projected` is not stuck behind a scrape drain. Retry Login V2 once,
  skip admin force-archive when the read model is empty, and wait for
  Send to re-enable between chat history seeds.

  Fixes [[incidents/e2e-ui-playwright-live-flake]]

  * feat: add cell host adapter and aggregate cell class

  Second portable-command host: CausalWorkspace talks to a per-shard
  CellStreamStore (in-process stand-in for private SQLite, not sqlx and
  not a celld Cargo feature). AggregateCell mounts the same PortableCommand
  declarations as SOA Routes and dispatches them without GraphQL or
  projectors.

  Implements [[tasks/portable-command-hosts-4]]

  * feat: parent-shard game cells for Blob and bomberman tick

  CellStreamStore::for_parent_shard holds sibling streams (map, player,
  bomb) in one cell SQLite and one CommitBatch. Bomberman tick shards by
  game id (`game:{game_id}`), not player/bomb. Blob cells stay
  `blob:{game_id}`. There is no two-cell transaction API.

  Implements [[tasks/portable-command-hosts-5]]

  * test: add celld compose host and TodoCell worker

  One SQLite Durable Object class per todo id, official celld image via
  Docker Compose. Fixture tests always run; live HTTP create/complete is
  gated on CELLD_URL. No MinIO, no celld Cargo feature, no secrets.

  Implements [[tasks/portable-command-hosts-6]]

  * test: run local celld compose against Azurite

  Azurite is the documented local bucket (az://celld). Docker Desktop
  injects extra_hosts, so celld cannot share Azurite's network namespace;
  socat forwards 127.0.0.1:10000 to the azurite service.

  Implements [[tasks/portable-command-hosts-6]]

  * feat: run Todo AggregateCell as workers-rs wasm on celld

  Replace the JS TodoCell with a workers-rs Durable Object that mounts
  todo-domain create/complete through AggregateCell. wasm32 uses a JS
  Date wall clock because SystemTime::now panics on unknown-unknown.

  Implements [[tasks/portable-command-hosts-7]]

  * feat: persist Todo cell event log in Durable Object SQLite

  CellStreamStore dumps EventRecords into the DO cell_events table and
  restores them on each request. GET after celld restart still hydrates
  the event-sourced Todo.

  Implements [[tasks/portable-command-hosts-8]]

  * feat: enable repository snapshots on the celld host

  AggregateCell can use with_snapshots; CellStreamStore implements
  SnapshotStore and get_stream_tail. Todo is Snapshottable. The worker
  persists cell_snapshots next to cell_events so load after restart is
  snapshot plus event tail.

  Implements [[tasks/portable-command-hosts-9]]

  * feat: public in-process causal invoke with receipt

  Make Service::dispatch_causal_with_receipt callable outside crate::microsvc
  and add an integration test that asserts payload plus receipt.

  Implements [[tasks/portable-command-hosts-10]]

  * feat: HTTP and gRPC causal wait-path with receipt

  POST /{command} and gRPC Dispatch accept { commandId, input } and return
  payload plus receipt. Identity comes from transport headers/metadata.
  Bus::send stays fire-and-forget.

  Implements [[tasks/distributed-command-surfaces-2]]

  * feat: GraphQL wait-path through CommandHost, not Service

  Mutations and status resolve via LocalCommandHost or HttpCommandHost.
  HTTP/WebSocket request data no longer carries Arc<Service>.

  Implements [[tasks/distributed-command-surfaces-3]]

  * feat: GraphQL CommandHost without Service unwrap

  graphql_router_with_dispatcher is a CommandHost; GraphQL-only
  engines wait-dispatch to HTTP writers. Task 20 mTLS stays the
  CMP envelope; wait-path remote is HttpCommandHost.

  Implements [[tasks/distributed-command-surfaces-3]]

  * feat: cell sealed row and command-named wait-path HTTP

  Persist GET sealed JSON next to events/snapshots. Todo cell POST
  /{command} with { commandId, input }. GET queues behind POST on
  the same isolate.

  Implements [[tasks/distributed-command-surfaces-4]]

  * feat: GraphQL ReadStore SQL scan vs cell GET-by-pk

  Mount store per model on the engine, not the ReadModel type.
  Cell-by-key compiles PK/by-id only and rejects list/filter/join.

  Implements [[tasks/distributed-command-surfaces-5]]

  * feat: optional celld+NATS e2e-ui profile

  Named profile under tests/e2e-ui/celld-nats-profile. Default
  one-process host.rs / make run is unchanged.

  Implements [[tasks/distributed-command-surfaces-6]]

  * fix: GraphQL command status test injects CommandHost

  authorized_unknown_status_returns_only_public_state no longer
  puts Arc<Service> in request data.

  Implements [[tasks/distributed-command-surfaces-3]]

  * chore: add make up-celld-nats for the optional profile

  make run stays the one-process playground. Bring-up, smoke, and
  teardown of celld+NATS are named targets.

  Implements [[tasks/distributed-command-surfaces-6]]

  * fix: make up-celld-nats tolerate an occupied NATS port

  Reuse a running compose NATS; if 14222 is taken by something
  else, print the listener and how to override NATS_PORT.
  down-celld-nats also removes a stray docker-run container.

  Implements [[tasks/distributed-command-surfaces-6]]

  * feat: serve GraphQL WS through graphql_router_with_host

  CommandHost routers need /graphql/ws for live chat. Export
  ProtocolResponseAccumulator so out-of-crate hosts can implement
  CommandHost, and let wait-path clients remap payload JSON.

  Implements [[tasks/distributed-command-surfaces-7]]

  * feat: add e2e-celld GraphQL host wait-dispatching Todo to celld

  Sibling example of e2e-ui (not make run). New todo/chat/blob/graphql
  service crates reuse the e2e-ui domain crates. Todo create/complete
  go through HttpCommandHost to {CELLD_URL}/todo/{id}/{command}; SQL
  lists dual-write locally so the playground UI can render.

  Implements [[tasks/distributed-command-surfaces-7]]

  * docs(e2e-ui): point optional celld profile at sibling example

  Navbar shows a CELLD badge when PUBLIC_E2E_PROFILE=celld-nats.
  make run stays the one-process playground.

  Implements [[tasks/distributed-command-surfaces-7]]

  * feat: add portable_command! for domain command mounts

  PCH-DEC-001 asked for a macro beside the Routes builder. #[command]
  already exists, so the function-like form is portable_command!. Todo
  thin commands (complete, rename, reopen, archive, purge) expand to
  shard + invoke + Eventual. create and force_archive keep handle:.

  Implements [[tasks/portable-command-hosts-2]]

  * refactor(e2e-celld): move Zitadel auth into an identity service crate

  Chat is lobby posts only. Identity owns ingress, scrape, and the
  AuthUsers projector on its own outbox leaf — not the chat aggregate.

  Implements [[tasks/distributed-command-surfaces-7]]

  * docs: explain celld vs one-process hosts on home and README

  Stack badges, portable_command! walkthrough, and both make run recipes.
  Domain declarations stay the same; the celld host wait-dispatches Todo.

  Implements [[tasks/distributed-command-surfaces-7]]

  * feat(e2e-celld): drain ChatCell outbox through MessagePublisher

  Wait-path returns events+outbox from the cell SQLite. GraphQL publishes
  via MessagePublisher (NATS here), fire-and-forgets outbox.complete, and
  seals Eventual projection metadata from those occurrences without a
  second command write. Chat @live stays on the GraphQL process.
  e2e-ui make run is unchanged.

  Implements [[chat-celld-wait-path-keeps-graphql-live]]
  Implements [[tasks/distributed-command-surfaces-7]]

  * refactor: lift celld CommandHost into the framework

  CelldCommandHost and cell outbox drain live in distributed::cell_host.
  Aggregate crates only supply CelldRoute. GraphQL is the user OIDC edge
  (engine OidcBearer); the Tower JWT-to-header layer is gone. make run
  cargo-watches GraphQL and the worker.

  Implements [[chat-celld-wait-path-keeps-graphql-live]]
  Implements [[tasks/distributed-command-surfaces-7]]

  * feat(e2e-celld): use Postgres for GraphQL read models

  The example host no longer falls back to sqlite:./e2e-celld.db. DATABASE_URL
  comes from e2e-ui.env (make -C tests/e2e-ui up). Cells still keep private
  SQLite per Durable Object.

  Implements [[chat-celld-wait-path-keeps-graphql-live]]

  * fix(replica): keep SPA nav and Eventual lists from flashing stale SQL

  Fence Eventual projection-delta rows so a later complete @live snapshot
  cannot drop them after Delivered. Skip GraphQL SSR seeds on SvelteKit
  isDataRequest so client navigations use the replica; hover prefetches
  the route operation in the browser.

  Implements [[specs/e2e-ui/sveltekit-dx]]

  * ci: run celld and e2e-celld tests on PRs and main

  Add integration-celld.yaml: e2e-celld workspace tests plus live
  Azurite+celld+NATS (`make test-celld`). Wire it into the PR and main
  gates so live HTTP no longer skips without CELLD_URL.

  Implements [[tasks/portable-command-hosts-11]]

  * fix(ci): unblock js-client, quality, all-features, and chat e2e

  Pack-smoke now lists matchDistributedRoute. Snapshot tail loads clamp
  prefix to the durable stream so a planted-ahead cache misses and
  replays. CausalDispatchResult/OutboxMessage implement PartialEq so
  graphql lib tests compile. Chat Send no longer stays disabled while
  Eventual projected is still catching up.

  Implements [[tasks/portable-command-hosts-11]]

  * fix: snapshot tail loads, Eventual projected, and chat Send

  Keep snapshot-only SQLite loads when event rows were deleted; clamp
  prefix only when a stream version exists so planted-ahead cache still
  misses. Tail-only hydrate keeps post-snapshot events in memory.

  Eventual `projected` settles when a committed result frame names the
  command (or has no command payload), even if membership fences keep the
  list overlay. Chat Send is disabled only while busy so it re-enables
  after projected with an empty composer.

  Implements [[tasks/portable-command-hosts-11]]

  * fix(js): settle Eventual projected on command-free live frames only

  Settling projected whenever a live frame named the command made
  status-regression tests miss their rejection. Query/@live frames have
  no command payload; those still settle so chat Send can clear busy
  while membership fences keep the overlay.

  Implements [[tasks/portable-command-hosts-11]]

  * fix(replica): keep Atomic membership fences off @load lists

  Atomic direct projection was taking Eventual list membership fences.
  Blob, new games, and client-side navigations never get @live, so those
  fences rejected later complete snapshots and the UI waited seconds.

  Eventual chat still fences list membership so stale complete @live cannot
  drop a posted row. Chat Send stays disabled while busy or empty; tests
  wait for enabled after fill so Playwright binds the draft.

  Implements [[chat-celld-wait-path-keeps-graphql-live]]
  Implements [[tasks/portable-command-hosts-11]]

  * fix: keep Eventual projectors live and fill auth_users on scrape

  Postgres listen/subscribe drained to idle and rebuilt Service on every
  quiet stretch, so chat projection lagged several seconds. Long-running
  hosts now idle-poll instead of exiting the consumer.

  Zitadel scrape treated duplicate outbox ids as done and skipped the
  directory upsert. Those events never reached bus_log, so auth_users
  stayed empty and chat author names disappeared. Scrape now writes
  auth_users from the Management API profile even when outbox already
  has the delivery.

  Implements [[chat-celld-wait-path-keeps-graphql-live]]
  Implements [[tasks/portable-command-hosts-11]]

  * fix(celld): make command lifecycle durable and fast

  Keep long-running consumers alive across idle polls, route every Todo transition to one cell, persist fenced cell command replays, enforce CellByKey row policies, and run the real browser lifecycle in celld CI.

  Refs [[incidents/pr-206-e2e-ui-command-latency-1]]

  * fix(replica): avoid stale celld revalidation races

  Trust exact authenticated projection deltas instead of refetching solely because they have no obligations. Preserve conservative revalidation for unconditional recovery cases, cover rapid Todo transitions, and keep newly-created Todo controls pending until the durable receipt arrives.

  * fix(replica): preserve soft-navigation authority

  Install the anonymous public Chat client during client-side route entry even when SvelteKit omits data-request hydration. Atomically seal locally provable collection membership from authoritative direct command rows so Blob start is visible without refresh, while leaving unprovable membership stale.

  * fix: settle exact command projections without refetch

  * fix(celld): harden command and outbox boundaries

  * fix(celld): use wasm-compatible claim clock

  * test(celld): authenticate optional profile reads

  * test(celld): harden local live smoke parity

  * refactor(e2e-ui): organize portable commands


See full diff: [v4.2.0...v4.3.0](https://github.com/hops-ops/distributed/compare/v4.2.0...v4.3.0)
