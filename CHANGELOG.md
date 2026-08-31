### What's changed in v4.8.0

* feat(e2e-ui): local gitops (#214) (by @patrickleet)

  * feat: split local and cloud gitops charts

  Generate independent local and deploy workload charts, optional test-user charts, and cloud-only promotion forwarding. Update the generic e2e-ui fixture to the canonical local Environment contract.\n\nImplements [[tasks/gitops-promotion-chart-contract]]

  * feat(e2e-ui): align local GitOps environment

  * refactor: keep test-user charts project-owned

  * fix(e2e-ui): resolve identity IDs through Crossplane

  * fix: address local gitops review findings

  * fix(cli): render cloud image guard correctly

  * test(e2e-ui): exercise local gitops in kind

  * fix(e2e-ui): build wasm before local gitops

  * fix(e2e-ui): route zitadel login in local gitops ci

  * fix(e2e-ui): build app before local gitops ci

  * test(e2e-ui): wait for local oidc redirect

  * test(e2e-ui): capture Kubernetes app logs on failure

  * test(e2e-ui): wait for local API readiness

  * test(e2e-ui): use lifecycle-owned JS preparation

  * fix(e2e-ui): publish Kubernetes diagnostics

  * test(e2e-ui): wait for API listener before forwarding


See full diff: [v4.7.0...v4.8.0](https://github.com/hops-ops/distributed/compare/v4.7.0...v4.8.0)
