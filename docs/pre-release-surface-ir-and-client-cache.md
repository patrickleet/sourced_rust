# Pre-release: surface IR + client cache

**Branch:** `feat/surface-ir-and-client-cache`  
**Merges into:** `tasks--graphql-qs-epic` (not `main`)

Normative designs live in **GitKB** (not this file):

| Topic | Spec | State gaps | Epic |
|-------|------|------------|------|
| Shared GraphQL surface IR | `specs/query-layer/v1/surface-ir` | `specs/query-layer/v1/state` A* | `tasks/graphql-qs-surface-ir-1` |
| Browser query cache + optimistic commands | `specs/query-layer/v1/client-cache` | `specs/query-layer/v1/state` B* | `tasks/graphql-qs-client-cache-1` |
| Sequencing | — | — | `tasks/graphql-qs-prerelease-1` |

**Server invariant unchanged:** GraphQL never writes read-model tables. Optimistic
updates are **browser cache only**.
