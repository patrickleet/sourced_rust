# distributed_cli (`dctl`)

Service tooling for [Distributed](https://crates.io/crates/distributed): a `dctl`
binary — and a library — that scaffolds service crates, inspects a service's
logical ApplicationManifest, and renders physical read-model schema artifacts
(SQL or an Atlas Operator resource).

```bash
cargo install distributed_cli   # installs the `dctl` binary
```

It is also a library, so another CLI can mount its commands instead of
reimplementing them. `hops`, for example, exposes the same surface under
`hops service` by depending on this crate and dispatching with
`distributed_cli::run`. **Everything below documented as `dctl <cmd>` is also
available as `hops service <cmd>`.**

## `dctl scaffold <name>` — generate a service crate

```bash
dctl scaffold orders --store postgres --transport http --gitops
```

Writes a ready-to-build Distributed service under `./<name>` (override with
`--path`). Common flags: `--store <postgres|sqlite|in-memory>`, `--transport
<http|knative>`, `--model <name>` (repeatable), `--read-models`, `--command` /
`--event` (repeatable), `--bus <rabbitmq|kafka|psql|nats>`, `--gitops`,
`--metrics prometheus`, `--tracing` / `--otel`, `--gitops-promote <argo|flux>`,
`--github OWNER/REPO`, `--force`. See `dctl scaffold --help` for the full list.

When used with `--gitops`, `--metrics prometheus` emits Prometheus Operator
`ServiceMonitor` and `PrometheusRule` templates for HTTP services. The
generated values default both resources to disabled; enable them only in
clusters with the Prometheus Operator CRDs installed. Plain `--gitops` does not
emit `monitoring.coreos.com` resources.

`--tracing` enables Distributed's optional `otel` span feature, emits a default
OTLP tracing setup in the generated `main.rs`, and renders OTLP environment
values in the Helm chart without hard-coding an endpoint.

## `dctl skills init` — extract agent skills into a project

```bash
dctl skills init                    # writes ./.distributed/skills/ and wires harnesses
dctl skills list                    # names + descriptions of the embedded skills
```

Materializes the **agent skills** embedded in the binary — markdown guidance
for coding agents on using Distributed (`distributed-usage`, `distributed-ci`,
`distributed-schema`) — into `.distributed/skills/<name>/SKILL.md` (override
the container with `--path <dir>`, which yields `<dir>/skills/...`). No network
and no repo checkout: the binary that scaffolded your service carries the
matching guidance for it.

`--agents <list>` wires the skills for native discovery by agent harnesses.
The canonical files live under the container; each harness location gets a
**per-skill symlink** to the canonical folder (a real copy on platforms
without reliable symlinks), anchored at the container's parent directory —
one on-disk copy, and your own skills coexist next to the links:

| value | effect |
|------|--------|
| `auto` (default) | wire every harness with evidence in the project root (`.claude/` → claude; `AGENTS.md`/`.agents/`/`.gemini/`/`.pi/` → agents); a fresh project wires both |
| `claude` | link each skill at `.claude/skills/<name>` (Claude Code) |
| `codex`, `grok`, `openai`, `gemini`, `pi`, `agents` | link each skill at `.agents/skills/<name>` (Codex, Grok Build, Gemini CLI, Pi) and maintain a sentinel-delimited managed block in `AGENTS.md` (created if absent); user content outside the sentinels is preserved |
| `none` | canonical `.distributed/skills/` files only |

Re-runs are safe and idempotent — per file: absent → `created`, identical →
`unchanged`, locally edited → skipped with a warning (`--force` to overwrite,
printed as `updated`). A harness path that is not a link to the canonical
folder (a stale link, or a directory from an older copy-based layout) is
likewise skipped unless `--force` replaces it. Files you add under the skills
directories are never touched. After a CLI upgrade, re-run with `--force` to
refresh existing skill files to the binary's embedded content; without
`--force`, differing files are treated as local edits and skipped.

## The artifact entrypoints

`describe` compiles your service crate and calls the explicit logical
application-manifest entrypoint — by default `<crate>::application_manifest`.
`schema` calls the separate physical read-model catalog entrypoint — by default
`<crate>::read_model_catalog`. Keep those owners separate:

```rust
use distributed::{ReadModelCatalog, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, ReadModel)]
#[table("orders")]
pub struct OrderView {
    #[id("order_id")]
    pub order_id: String,
    pub status: String,
}

pub fn read_model_catalog() -> ReadModelCatalog {
    ReadModelCatalog::new("orders").read_model::<OrderView>()
}
```

Point at a different function with `--entrypoint <path>`. Because these commands
compile the target crate, they need the local `distributed` crate to be
resolvable — found automatically from the workspace, or pass `--distributed-path`
/ set `DISTRIBUTED_PATH`.

## `dctl client-manifest` — authorized client surface

```bash
dctl client-manifest > target/distributed-client.json
```

Compiles the service's `distributed_client_surface` export into the versioned,
role/application-selected manifest used by the operation compiler. The export
already contains one concrete role or named application surface; it is not an
admin catalog that downstream tools filter themselves.

## `dctl client` — typed query, live, and command artifacts

```bash
dctl client \
  --manifest target/distributed-client.json \
  --role user \
  --documents 'src/**/*.graphql' \
  --out src/generated/distributed

# CI: parse, validate, and compare without writing
dctl client \
  --manifest target/distributed-client.json \
  --role user \
  --documents 'src/**/*.graphql' \
  --out src/generated/distributed \
  --check
```

The requested `--role` or `--surface` must exactly match the manifest's
authorized identity. Generation validates the GraphQL documents, injects only
authorized wire-only identity metadata, derives an exact live companion for
`@live`, and emits framework-neutral TypeScript replica artifacts alongside the
manifest-owned command and protocol operations. Operation IDs hash the exact
full document sent over GraphQL; they do not imply an APQ/persisted-operation
registry.

Each operation artifact also contains the closed variable/input codec compiled
from that selected surface. The runtime applies GraphQL singleton-list coercion,
canonical scalar encoding, unknown-field rejection, and deterministic deep
freezing before variables can identify a cache entry or reach the network.
Manifest v7 and variable codec v2 carry the selected service's exact
`max_depth`, `max_bool_width`, and `max_in_list` contract. Static literal and
mixed-variable filters are rejected during generation when their known shape
exceeds those limits; runtime variables carry per-use `filterBaseDepth` and
`maxItems` constraints. Reused variables receive the most restrictive
intersection across every root, relationship selection, and aggregate use.
The current query surface supports complete and offset-window roots; cursor
artifacts are not certified and therefore fail closed to revalidation.

`@load` is discovered automatically for `src/routes/**/+page.graphql`. A
co-located document with a different filename can use the explicit fallback
`--route OperationName=/route`. Unsupported or unprovable selections fail at
build time with their source location; the compiler does not emit a partial
normalization plan.

## `dctl describe` — manifest as JSON

```bash
dctl describe                       # current directory
dctl describe --manifest-path path/to/Cargo.toml --package orders-service
```

Prints the versioned manifest envelope (schemas, services, transports) as JSON —
a stable contract for other tooling.

## `dctl schema` — schema artifacts

Renders the **desired-state** schema for the manifest's read models and
operational tables. Output goes to stdout by default (or `--out <file>`).

### SQL (default)

```bash
dctl schema --dialect postgres      # or --dialect sqlite
```

### Atlas Operator resource (`--format atlas`)

Wraps the desired-state SQL into an `AtlasSchema` (`db.atlasgo.io/v1alpha1`) for
the [ariga atlas-operator](https://github.com/ariga/atlas-operator), so the
operator diffs the live database against it and applies the migration in-cluster.

The resource is written to **stdout** — `dctl` deliberately does not pick a
location for it. Redirect it wherever you keep schema manifests: a file in the
service repo, or a separate GitOps/schema repo.

```bash
dctl schema --format atlas \
  --name orders \
  --namespace data \
  --db-secret orders-db \
  --db-secret-key url \
  > orders.schema.yaml
```

| flag | maps to |
|------|---------|
| `--name` | `metadata.name` (required; RFC-1123 label) |
| `--namespace` | `metadata.namespace` (optional) |
| `--db-secret` / `--db-secret-key` | `spec.urlFrom.secretKeyRef` — the GitOps-friendly choice, no credentials in the manifest (`--db-secret-key` defaults to `url`) |
| `--db-url` | inline `spec.url` — convenient for dev; avoid committing real credentials |
| `--dev-url` | `spec.devURL` — a scratch database Atlas uses to plan changes |
| `--dialect` | SQL dialect of the wrapped schema (`postgres` default) |

Provide a database reference via **either** `--db-secret` or `--db-url` (not
both). Example output:

```yaml
apiVersion: db.atlasgo.io/v1alpha1
kind: AtlasSchema
metadata:
  name: orders
  namespace: data
spec:
  urlFrom:
    secretKeyRef:
      name: orders-db
      key: url
  schema:
    sql: |
      CREATE TABLE IF NOT EXISTS "orders" (
        ...
      );
```

Names are validated as RFC-1123 labels (lowercase letters, digits, hyphens; no
leading/trailing hyphen) so generation fails with a clear message rather than
emitting YAML the API server would reject.
