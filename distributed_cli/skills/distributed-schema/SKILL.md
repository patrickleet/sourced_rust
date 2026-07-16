---
name: distributed-schema
description: Inspect a Distributed service manifest and render schema artifacts - dctl describe (manifest JSON), dctl schema (migration SQL or an Atlas Operator resource), and the distributed_manifest() envelope contract. Use when working on read-model schemas, migrations, or schema automation.
---

# Manifests and schema artifacts

`dctl describe` and `dctl schema` are the schema toolchain for a Distributed
service. Both **compile the target crate** and call its exported manifest
function — by default `<crate>::distributed_manifest` (override with
`--entrypoint <path>`).

## The manifest entrypoint

The service must export a function that registers its read models / tables:

```rust
use distributed::{DistributedProjectManifest, ReadModel};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("orders")]
pub struct OrderView {
    #[id("order_id")]
    pub order_id: String,
    pub status: String,
}

pub fn distributed_manifest() -> DistributedProjectManifest {
    DistributedProjectManifest::new("orders").read_model::<OrderView>()
}
```

A read model not registered here is invisible to `describe`/`schema` — no SQL
is rendered for it. When you add a `#[derive(ReadModel)]` type, register it.

## `dctl describe` — manifest JSON

```bash
dctl describe                                              # current directory
dctl describe --manifest-path path/to/Cargo.toml --package orders-service
```

Prints the versioned manifest envelope. The contract other tooling relies on:
a numeric `schema_version` (currently `1`) and a `project` object. `dctl`
rejects envelopes with a missing/different `schema_version`, so treat the
envelope as a stable machine interface, not free-form JSON.

## `dctl schema` — SQL or Atlas resource

Renders the **desired-state** schema for the manifest's read models and
operational tables. Output goes to stdout (or `--out <file>`).

```bash
dctl schema --dialect postgres        # migration SQL (or --dialect sqlite)
```

### Atlas Operator resource

`--format atlas` wraps the SQL in an `AtlasSchema` (`db.atlasgo.io/v1alpha1`)
for the ariga atlas-operator, which diffs the live database and applies the
migration in-cluster — declarative schema via GitOps:

```bash
dctl schema --format atlas \
  --name orders \
  --namespace data \
  --db-secret orders-db \
  > orders.schema.yaml
```

- Database reference is **either** `--db-secret <name>` (renders
  `spec.urlFrom.secretKeyRef`; key defaults to `url`, override with
  `--db-secret-key`) **or** `--db-url <url>` (inline `spec.url`) — never both.
  Prefer `--db-secret` anywhere the YAML is committed: no credentials in git.
- `--name` is required and validated as an RFC-1123 label (lowercase
  alphanumerics and hyphens); generation fails fast rather than emitting YAML
  the API server would reject.
- `--dev-url` sets `spec.devURL`, the scratch database Atlas uses to plan
  changes.
- The resource goes to **stdout** deliberately — redirect it to wherever schema
  manifests live (service repo or a GitOps repo). `dctl` does not pick a
  location.

## Running in CI

Because `describe`/`schema` compile the service crate, CI needs the crate's
dependencies resolvable:

- A published `distributed` dependency works as-is.
- A **path dependency** on a local `distributed` checkout needs
  `--distributed-path <dir>` or `DISTRIBUTED_PATH=<dir>` in the environment
  (auto-discovery walks ancestor directories, which usually fails in CI
  checkouts).

Typical schema-gate step: render and diff so schema drift fails the build.

```bash
dctl schema --dialect postgres --out rendered.sql
git diff --exit-code rendered.sql
```

Or regenerate the Atlas resource and let the GitOps PR carry the change:

```bash
dctl schema --format atlas --name orders --db-secret orders-db \
  --out manifests/orders.schema.yaml
```

## Gotchas

- Whole-view/semistructured state belongs in a **declared read-model table**
  with `#[jsonb]` columns — SQL repositories do not persist generic document
  rows.
- Composite keys and indexes are declared in the derive:
  `#[readmodel(table = "...", primary_key = ["a", "b"])]`, `#[index(...)]`.
- Rendered SQL is desired-state, not a diff. Diffing against the live database
  is the Atlas operator's job (`--format atlas`) or your migration tool's.
- `dctl schema` and `dctl describe` also ship as `hops service schema` /
  `hops service describe`.

## Reference

Full flag table: `distributed_cli/README.md` in the Distributed repo
(https://crates.io/crates/distributed_cli). Event-store internals:
`README (Postgres repository section) and migrations/`; read-model metadata: `README § Read Models`.
