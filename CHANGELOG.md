### What's changed in v4.9.0

* feat: add coherent application hot reload (#219) (by @patrickleet)

  * feat: add coherent application hot reload

  * fix: prepare linked JavaScript before validation

  * test: cover lifecycle package exports

  * fix: allow bounded cold Vite readiness

  * test: recognize staged linked package builds

  * fix: preserve browser network idle during dev

  * fix: make reload proof observe durable result

  * test: make readiness timing bounds scheduler-safe

  * test: retain reload preparation across navigation

  * fix: avoid lifecycle compiler acknowledgement deadlock

  * fix: close coherent reload review gaps

  * test: prove reads remain available during reload

  * fix: close coherent reload edge cases

  * fix: match Vite HTML transform types

  * feat: unify local GitOps application lifecycle

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * ci: install chart contract dependency

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * fix: ship required WASM compiler with framework

  Resolve declared pures from local path dependencies so sibling application workspaces build through the same lifecycle.

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * fix: map combined UI OIDC credentials correctly

  Add a rendered chart contract for the adjacent client-id and client-secret mappings.

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * ci: prove framework-owned WASM toolchain

  Remove host wasm-pack installation from e2e-ui and celld jobs so application builds exercise the compiler shipped through the framework package.

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * fix: defer supervised Vite compilation to lifecycle

  Serve staged generated clients during distributed dev and suppress compiler-input HMR so the outer coherent lifecycle remains the sole compiler owner.

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * fix: allow cold runtime readiness in dev

  Keep readiness bounded while permitting cargo run to populate a fresh target directory in worktrees and development containers.

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * fix: include watcher in readiness budget

  Allow the bounded discovered API and UI probe budgets plus the framework watcher's startup delay.

  Implements [[tasks/distributed-gitops-coherent-lifecycle]]

  * fix: support lifecycle clients on HTTP origins


See full diff: [v4.8.0...v4.9.0](https://github.com/hops-ops/distributed/compare/v4.8.0...v4.9.0)
