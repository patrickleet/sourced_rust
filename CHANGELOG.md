### What's changed in v4.6.0

* feat: add coherent application build and dev lifecycle (#213) (by @patrickleet)

  ##### Summary

  This PR makes Distributed application development a project command instead of a lifecycle-configuration exercise:

  ```bash
  cd my-application
  distributed build
  distributed dev
  ```

  The CLI discovers the Cargo workspace, typed application export, runtime binary, and conventional `ui/` SvelteKit project. Application authors do not create `distributed.contracts.json`, `distributed.lifecycle.json`, executor lists, readiness commands, or lifecycle shell scripts. The generated state belongs to the tool under `.distributed/lifecycle/`.

  This also keeps the framework modular. Projects can continue using only Distributed's lower-level primitives. The build/dev experience in this PR applies when a project composes those pieces into a typed `ApplicationManifest`; a SvelteKit UI is optional.

  ##### How it works

  ```mermaid
  sequenceDiagram
      actor Author
      participant Build as distributed build + Vite
      participant Kit as SvelteKit
      participant API as GraphQL gateway
      participant Domain as Command handler + aggregate
      participant Projector as Projection worker
      participant DB as Read-model database
      participant Replica as Browser replica

      rect rgb(245, 247, 255)
          Note over Author,Replica: Generate one application from authored Rust and page GraphQL
          Author->>Build: Domain crates: commands, events, command RBAC
          Author->>Build: Read-model crates: query shapes, relationships, read RBAC
          Author->>Build: Projections: event → internal read-model mutation
          Author->>Build: +page.graphql: @load and optional @live
          Build-->>API: Command + query/subscription surfaces with RBAC
          Build-->>Projector: Server projection programs
          Build-->>Kit: Loaders, live operations, typed commands, replica plans
          Build-->>Replica: Optimistic projection programs + declared Rust/WASM pures
      end

      rect rgb(245, 255, 247)
          Note over Kit,Replica: Initial page render — @load
          Kit->>API: Authorized @load query (SSR or navigation)
          API->>DB: Read the authorized model slice
          DB-->>API: Rows + identities + revisions
          API-->>Kit: Query result
          Kit->>Replica: Normalize, dehydrate, hydrate + server authority
          Note right of Replica: Browser does not repeat the first query
          Replica-->>Kit: Reactive confirmed snapshot
      end

      rect rgb(255, 251, 240)
          Note over Kit,Replica: Ongoing page updates — @live
          Kit->>Replica: Generated operation attaches live automatically
          Replica->>API: Subscribe with the same query + variables
      end

      rect rgb(255, 245, 250)
          Note over Kit,Replica: Write path — public domain command, never public model mutation
          Kit->>Replica: Generated typed command + input
          Replica-->>Kit: Apply predicted projection immediately
          Replica->>API: Authorized domain command
          API->>Domain: Execute command
          Domain-->>Projector: Commit domain event
          Projector->>DB: Apply internal projection mutation
          DB-->>API: Publish committed read-model change
          API-->>Replica: @live records + causal clocks
          Replica-->>Kit: Confirm or reconcile the optimistic snapshot
          Note over Projector,Replica: The generated projection protocol drives both server updates and browser optimism
      end
  ```

  At the SvelteKit boundary, a co-located `+page.graphql` document enters generation with the typed Rust application. `@load` populates a request-local server replica and dehydrates its authorized route seed into the browser without a duplicate first request. `@live` attaches automatically from the generated operation and carries committed records plus causal clocks back to the browser replica. Domain commands can update that replica optimistically while the same projection protocol updates committed server read models.

  ##### Application model

  The typed Rust composition preserves CQRS responsibilities instead of deriving one CRUD API from domain structs:

  The manifest is normal typed Rust composition—not author-maintained lifecycle JSON. New scaffolds create the export and existing projects can compose it from their real modules and surfaces. Two checked-in examples now exercise the same CLI path:

  - [e2e-ui `application_manifest`](https://github.com/hops-ops/distributed/blob/feat/application-lifecycle-build-dev/tests/e2e-ui/crates/service/src/modules/graphql.rs#L139) — conventional discovery with a SvelteKit `ui/`
  - [e2e-celld `application_manifest`](https://github.com/hops-ops/distributed/blob/feat/application-lifecycle-build-dev/tests/e2e-celld/crates/graphql-service/src/modules/graphql.rs#L139) — explicit package/runtime metadata in a multi-crate, API-only project

  | Authored source | API/runtime responsibility |
  |---|---|
  | Domain aggregates and handlers | Command APIs and command RBAC; handlers execute domain commands and emit domain events |
  | Read-model structs | GraphQL query/subscription surfaces and read RBAC |
  | Projections | Domain-event → internal read-model mutation programs, applied to committed server records and browser replica slices |
  | Declared Rust/WASM pure functions | Required browser artifacts compiled from their declaring Cargo package for optimism that cannot be predicted from command inputs alone |
  | SvelteKit `+page.graphql` documents | Generated `@load` SSR/navigation loaders and `@live` subscriptions over the same authorized read-model operation |
  | Service/application composition | Chooses which modules run together as one service or several, leaving the CAP trade-offs explicit |

  GraphQL mutation syntax is internal projection IR here; it is not permission for public clients to mutate domain models directly. Public writes remain domain commands. Vite uses the typed application surface to generate the client and optimistic-replica artifacts.

  ##### What an author sees

  ###### Build the current project

  ```bash
  $ cd tests/e2e-ui
  $ distributed build

  distributed build: compiling Rust runtime e2e-ui (e2e-runner)
  distributed build: validating typed application e2e-service through e2e_service::application_manifest
  distributed: compiling required browser WASM blob/pkg/blob_wasm from Cargo package blob-domain
  distributed build: compiling SvelteKit UI .../tests/e2e-ui/ui
  distributed build: introspecting typed application e2e-service
  distributed build: project=e2e-ui application=e2e-service runtime=e2e-ui ui=ui
  lifecycle graph: ok generation=sha256:... release=sha256:... nodes=1
  ```

  From another directory, the project is a positional argument:

  ```bash
  distributed build ./tests/e2e-ui
  distributed build ./tests/e2e-ui --check --output json
  ```

  `distributed build`:

  1. reads the workspace model with `cargo metadata`;
  2. resolves the typed application and runtime from scaffold-owned Cargo metadata or unambiguous conventions;
  3. compiles the Rust runtime;
  4. validates the real typed `ApplicationManifest` before starting the UI build;
  5. compiles every declared browser WASM pure from its declaring Cargo package;
  6. installs missing UI dependencies and runs the SvelteKit/Vite build when `ui/package.json` exists;
  7. reuses the cached introspection harness and atomically activates an immutable, content-addressed application generation only after every program build succeeds.

  Rust binaries remain Cargo outputs and SvelteKit uses its adapter-selected output. Lifecycle receipts, active-generation state, and the generated application manifest are internal CLI state under `.distributed/lifecycle/`.

  `--check` rebuilds typed application metadata in isolation, compares it with the active generated manifest, emits drift ownership in JSON when requested, and does not activate or rewrite anything.

  ###### Run the current project

  ```bash
  $ distributed dev

  distributed dev: project=e2e-ui api=e2e-ui ui=ui (Ctrl-C to stop)
  lifecycle dev: process api ready http://127.0.0.1:8791
  lifecycle dev: process ui ready http://localhost:5180
  lifecycle dev: ready generation=sha256:... processes=api,ui (Ctrl-C to stop)
  ```

  `distributed dev`:

  - compiles every declared browser WASM pure before Vite starts, and rebuilds it after relevant Rust changes;
  - activates the initial typed application generation before serving;
  - starts `cargo run` for the discovered runtime and `npm run dev` for `ui/`;
  - uses bounded, framework-neutral TCP readiness checks and prints usable URLs;
  - loads `<project-name>.env` and `.env` while preserving explicit shell environment values;
  - leaves Svelte/CSS/module hot updates to Vite;
  - watches typed Rust application inputs and restarts the API after a successful replacement generation;
  - terminates both process groups and their descendants on Ctrl-C, including bounded TERM/KILL escalation.

  Defaults are `BIND=127.0.0.1:8791`, `UI_HOST=localhost`, and `UI_PORT=5180`. Projects can override those normally through their shell or dotenv file.

  ##### Benefits and the features that provide them

  | Benefit | Feature |
  |---|---|
  | A new contributor can build or run a project without learning internal lifecycle files | Cargo metadata + project convention discovery |
  | Rust remains the semantic source of truth | Typed `ApplicationManifest` introspection; no Rust source scan or duplicated JSON inventory |
  | Command and query responsibilities do not collapse into CRUD | Domain-derived commands; read-model-derived queries/subscriptions; projection-derived replica changes |
  | A failed program build or manifest generation cannot advance active application metadata | Success barrier plus immutable generation activation |
  | Declared browser pures require no app-owned build scripts | `portable_command!` records the declaring Cargo package; build/dev run `wasm-pack` before Vite |
  | Frontend development keeps native HMR speed | Vite owns UI HMR; lifecycle supervision restarts only the Rust runtime for typed application changes |
  | Local auth/database settings work without sourcing a script on every run | Project dotenv loading with shell precedence |
  | Startup failures are understandable and fail before avoidable work | Typed-export preflight before Vite, visible Cargo/Vite output, explicit build phases, bounded compiler errors, and readiness URLs |
  | Ctrl-C does not leave Cargo, Vite, or readiness descendants behind | Process-group supervision and bounded shutdown |
  | CI tests the interface users invoke | Focused compiled-binary Bats coverage plus real e2e-ui and celld workflows entering through `distributed build` and `distributed dev` |

  ##### Scaffold and compatibility

  New `distributed scaffold` projects receive tool-owned Cargo metadata identifying their application entrypoint and runtime binary. Existing workspaces need no metadata when they have one conventional `*-service` library exporting `application_manifest` and one non-manifest runtime binary.

  The older file-driven lifecycle adapter remains available behind hidden `--root`, `--catalog`, and `--config` flags for compatibility and low-level graph tests. It is no longer the application-author workflow.

  The same internal command prefixing supports the embedded `hops service build` / `hops service dev` surface as well as the standalone binary.

  ##### Real e2e UI

  The checked-in e2e project now contains no lifecycle catalog, lifecycle config, placeholder manifest, or lifecycle preparation script.

  ```bash
  cd tests/e2e-ui
  make up           # Postgres + Zitadel + e2e-ui.env
  make ui-install   # once: build + install this checkout's locally linked JS package
  distributed build
  distributed dev
  ```

  Open:

  - UI: `http://localhost:5180`
  - GraphQL: `http://127.0.0.1:8791/graphql`

  This fixture links `../../../js` so CI tests the exact JavaScript framework source in the checkout; `@hops-ops/distributed` is also published to npm for ordinary applications. The link is repository test setup, not application lifecycle configuration. Declared Rust/WASM pures are required framework artifacts: `distributed build` and `distributed dev` discover their declaring Cargo packages and compile them before Vite, with no `make wasm` or application-owned build script.

  ##### CI and verification

  The integration workflow installs pinned Bats 1.14.0 and runs the compiled CLI. The suite passes locally:

  ```text
  1..4
  ok 1 project build and dev are zero-config and invalid typed exports fail before UI
  ok 2 build activates atomically and check reports drift without replacing active
  ok 3 dev reports process readiness, rebuilds selectively, and cleans descendants
  ok 4 Ctrl-C cancels the initial build before any process starts
  ```

  The integration workflows now prove the user-facing commands against two real application shapes:

  | CI scenario | Entry point | What it proves |
  |---|---|---|
  | e2e-ui offline | `distributed build .` | Compiles the real runtime, validates/introspects its typed manifest, compiles the declared Blob Rust/WASM pure, builds the SvelteKit adapter output, and then runs domain, suite, generated-client drift, type, and UI tests |
  | e2e-ui browser | `distributed build .` → `distributed dev .` | Supervises the real API and Vite processes; Playwright exercises auth, generated GraphQL `@load`/`@live`, commands, optimism, and browser behavior through their readiness URLs |
  | e2e-celld live | `distributed build tests/e2e-ui` + `distributed build tests/e2e-celld` + `distributed dev tests/e2e-celld` | Discovers explicit metadata in a nested multi-crate workspace, builds/serves an API-only project, and exercises it against celld, Queue relay, NATS, and the separately owned shared UI |

  The celld UI remains a separate Vite process because it physically belongs to `tests/e2e-ui`, not the API-only `tests/e2e-celld` project. This is an intentional optional-UI scenario, not a lifecycle bypass for the celld application.

  Additional verification:

  - e2e-celld `application_manifest_compiles_real_modules_and_surfaces` and its full GraphQL service suite — 10/10 passed
  - real `distributed build tests/e2e-celld --check --output json` — `ok: true`, no drift
  - `cargo test -p distributed_cli` — 215 unit tests plus all non-ignored CLI integration suites passed; lifecycle integration 9/9
  - `cargo clippy -p distributed_cli --all-targets -- -D warnings` — passed
  - real `distributed build tests/e2e-ui` — typed preflight, Rust, required `blob-domain` WASM, and SvelteKit/Vite builds passed with no lifecycle JSON; the activated manifest records `blob-domain` automatically
  - incompatible checkout reproduction — missing `application_manifest` failed before Vite, named the exact package/export contract, and preserved the active generation
  - real `distributed build tests/e2e-ui --check --output json` — `ok: true`, no drift
  - e2e `application_manifest_compiles_real_modules_and_surfaces` — passed
  - `git diff --check` — passed

  All prior CodeRabbit findings were individually audited, acknowledged as valid, fixed with regressions, replied to, and resolved. The review-driven fixes cover initial-build cancellation, bounded file enumeration, root-derived/cross-platform locking, submitted-snapshot correctness, unpredictable test roots, bounded live stderr, typed cancellation outcomes, generated-output glob dependencies, process-group cleanup, and bounded readiness probes.

  ##### Linked work

  GitKB tasks `coherent-build-dev-2`, `coherent-build-dev-3`, and `coherent-build-dev-4`. No GitHub issue was provided.


See full diff: [v4.5.0...v4.6.0](https://github.com/hops-ops/distributed/compare/v4.5.0...v4.6.0)
