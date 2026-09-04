### What's changed in v4.12.0

* feat: make client generation lifecycle-owned (#222) (by @patrickleet)

  * feat: generate clients in application lifecycle

  Implements [[tasks/dev-client-generation]]

  * refactor!: require lifecycle-owned client generation

  BREAKING CHANGE: SvelteKit client generation is only available through distributed build and distributed dev. Manual Vite helpers and committed client trees are removed.

  Implements [[tasks/dev-client-generation]]

  * fix: remove source-tree client contract ownership

  Generated client contracts are verified by the immutable lifecycle graph. Repository catalogs and reload probes no longer depend on deleted application-source outputs.

  Implements [[tasks/dev-client-generation]]

  * fix: defer activation for lifecycle checks

  Ensure discovered-project check requests never carry immediate activation authority. Non-check builds still validate the UI before explicit activation.

  * docs: correct public SvelteKit Vite surface

  * fix: stabilize lifecycle-owned client generation

  * test: assert stable lifecycle failure contract

  * test: allow cold coherent lifecycle rebuilds

  * test: bound cold lifecycle integration startup

  * fix: stabilize lifecycle integration startup

  * fix: keep lifecycle client ownership coherent

  * fix: harden lifecycle activation ordering

  * fix: use lifecycle-owned wasm compiler

  * fix: make lifecycle rollback process-safe

  * test: observe durable reload transitions

  * fix: keep lifecycle reload single-owner

  * fix: close lifecycle review gaps

  * ci: install browser before celld serving

  * fix: replay transient cell command failures

  * ci: keep GitOps browser tunnels alive


See full diff: [v4.11.0...v4.12.0](https://github.com/hops-ops/distributed/compare/v4.11.0...v4.12.0)
