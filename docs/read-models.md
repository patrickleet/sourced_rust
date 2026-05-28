# Read Models

Read models are query-optimized projection state stored as relational rows with
table, column, key, index, relationship, and schema metadata. Whole-view state
belongs in a declared table too: use an `id` column and JSON/JSONB columns for
semistructured fields.

The current implementation keeps these paths explicit:

| Path | API | Use when |
|---|---|---|
| Relational write mapping | `RelationalReadModel`, `ReadModelWritePlanBuilder`, `ReadModelWritePlan` | Normalized tables, composite keys, foreign keys, JSONB columns |
| Explicit relationship includes | `store.workspace().load(...).include(...).one()`, `sync` | Internal primary-key reads with declared one-level relationships |
| Schema lifecycle | `ReadModelSchemaRegistry`, `ReadModelSchemaAdapter` | Migration artifact generation, startup verification, explicit dev/test bootstrap |

## Relational Models

A model opts into relational metadata with `#[readmodel(table = "...")]` and
field attributes. Common table, column, id, index, and unique metadata also
have direct helper attributes:

```rust
use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[table("players")]
pub struct PlayerView {
    #[id("player_id")]
    pub id: String,
    pub display_name: String,
    #[readmodel(jsonb)]
    pub counters_by_game: std::collections::HashMap<String, i64>,
    #[readmodel(has_many = "PlayerWeaponView", foreign_key = "player_id")]
    pub weapons: Vec<PlayerWeaponView>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "player_weapons", primary_key = ["player_id", "weapon_id"])]
#[index(
    name = "idx_player_weapons_player_acquired",
    columns = ["player_id", "acquired_at"]
)]
pub struct PlayerWeaponView {
    #[readmodel(foreign_key = "players.player_id", delegated_from = "PlayerView.player_id")]
    pub player_id: String,
    pub weapon_id: String,
    #[index]
    pub acquired_at: String,
}
```

The derive emits `RelationalReadModel` metadata, row conversion, primary-key
metadata, JSONB column metadata, indexes, and an adapter-owned version column.
Composite and delegated keys are represented in the schema and in write-plan row
mutations.

Use `#[index]` or `#[index("index_name")]` for a secondary field index. Use
`#[unique]` or `#[unique("index_name")]` for a unique field index.
For compound indexes, put `#[index(columns = ["field_a", "field_b"])]` or
`#[unique(columns = ["field_a", "field_b"])]` on the struct. Add
`name = "..."` when the storage index name must be fixed.

## Explicit Relationship Includes

Relationship includes are primary-key anchored and opt-in. Register the
relational schemas with an adapter, load one root row, ask for each relationship
explicitly, mutate the hydrated struct, then sync the tracked workspace:

```rust
use sourced_rust::{InMemoryReadModelStore, ReadModelWorkspaceExt, RowKey, RowValue};

let store = InMemoryReadModelStore::new();
store.register_schema::<PlayerView>()?;
store.register_schema::<PlayerWeaponView>()?;

let mut workspace = store.workspace();
let mut player = workspace
    .load::<PlayerView>(RowKey::new([("player_id", RowValue::String("player-1".into()))]))
    .include("weapons")
    .one()?
    .expect("player should exist")
    .data;

player.display_name = "Ada Lovelace".into();
player.weapons.push(PlayerWeaponView {
    player_id: String::new(),
    weapon_id: "sword".into(),
    acquired_at: "2026-05-23".into(),
});

workspace.sync(player)?;
workspace.commit()?;
```

`has_many` relationships hydrate `Vec<T>` fields. `belongs_to` relationships
hydrate `Option<T>` fields.

`sync` makes storage match the struct: added items are inserted, changed
items updated, and **removed items deleted**. For an included `has_many`
relationship, dropping a child from the `Vec<T>` deletes that child row (the
loaded child set is complete, so the struct is the source of truth).
Added children have their delegated foreign keys filled before the write plan is
staged. Every change — including the delete — lowers to an explicit mutation in
the `ReadModelWritePlan`; nothing cascades to rows you did not load.

Clearing a `belongs_to` field to `None` is a no-op on the target: it never
deletes the owner, since other rows may reference it. To delete a whole root,
use `delete::<Root>(key)`.

This API is for command handlers, projectors, tests, admin tools, and adapter
conformance that need typed internal includes. It is not a public query DSL.
Hasura or another query gateway remains the intended public GraphQL/query API
for normalized Postgres read models.

## Command-Side Atomic Writes

Use `ReadModelWritePlanBuilder` when a command or projector stages multiple
row mutations. The current repository APIs are synchronous:

```rust
use sourced_rust::{
    ReadModelWritePlanBuilder, SyncCommitBuilderExt, SyncReadModelWritePlanCommitExt,
};

let mut read_models = ReadModelWritePlanBuilder::new();
read_models.upsert(&player)?;
read_models.upsert_related(&player, "weapons", &weapon)?;

repo.read_models_sync(read_models).commit_sync(&mut aggregate)?;
```

Async adapters can expose the same shape at their boundary:

```rust,ignore
use sourced_rust::{AsyncReadModelWritePlanCommitExt, ReadModelWritePlanBuilder};

let mut read_models = ReadModelWritePlanBuilder::new();
read_models.upsert(&player)?;

repo.read_models(read_models)
    .commit(&mut aggregate)
    .await?;
```

Builder ordering is semantic staging only. These forms are equivalent:

```rust
repo.read_models_sync(read_models)
    .outbox_sync(message)
    .commit_sync(&mut aggregate)?;

repo.outbox_sync(message)
    .read_models_sync(read_models)
    .commit_sync(&mut aggregate)?;

repo.aggregate_sync(&mut aggregate)
    .read_models_sync(read_models)
    .outbox_sync(message)
    .commit_sync()?;
```

## Standalone Distributed Projectors

A read-model service can commit a write plan without owning an aggregate
repository:

```rust
use sourced_rust::{ReadModelError, ReadModelWritePlanBuilder, ReadModelWritePlanStore};

fn project_message(
    store: &impl ReadModelWritePlanStore,
    event_id: &str,
    view: &PlayerView,
) -> Result<(), ReadModelError> {
    let mut read_models = ReadModelWritePlanBuilder::new();
    read_models
        .upsert(view)?
        .mark_processed("game-view-projector", event_id);

    let outcome = read_models.commit(store)?;
    if outcome.was_applied() || outcome.was_skipped() {
        // Ack the broker message after commit returns.
    }
    Ok(())
}
```

Processed-message marks are committed in the same adapter transaction as
read-model writes when the adapter advertises that capability. Duplicate
messages return a skipped outcome and do not apply the staged mutations again.

## Schema Registry And Bootstrap

Register relational models once and pass the registry to adapters:

```rust
use sourced_rust::ReadModelSchemaRegistry;

let mut registry = ReadModelSchemaRegistry::new();
registry
    .register::<PlayerView>()?
    .register::<PlayerWeaponView>()?;

registry.validate()?;
```

Adapters implement `ReadModelSchemaAdapter` to generate migration artifacts,
verify startup schema, or explicitly bootstrap dev/test schemas. Production
schema changes should be generated or user-authored migrations plus
verification; normal repository construction and command handling should not
silently sync production schemas.

SQL repositories expose the same lifecycle through neutral table-schema APIs so
read models and operational tables, such as the outbox table, use one metadata
path:

```rust
let mut registry = TableSchemaRegistry::new();
registry.register_schema(sourced_rust::outbox_message_schema())?;

let artifacts = repo.generate_table_migration_artifacts(&registry)?;
let bootstrap = repo.bootstrap_table_schema_for_dev(&registry).await?;
```

Use generated artifacts as migration input for production tooling such as Atlas.
Reserve `bootstrap_table_schema_for_dev` for tests and local development.

## Whole-View Rows

Bomberman `BoardView` is a read-model table row. It stores a whole game board
view with nested players, bombs, explosions, tiles, turn state, and counters in
declared columns, using JSON/JSONB for nested state.

If a command projection needs this shape, define a table with an `id` column and
JSON/JSONB columns for the board state:

```rust
#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "boards")]
pub struct BoardView {
    #[readmodel(id)]
    pub game_id: String,
    #[readmodel(jsonb)]
    pub players: Vec<PlayerState>,
    #[readmodel(jsonb)]
    pub bombs: Vec<BombState>,
}
```

It is still a relational row write.

## Non-Goals

The relational ORM slice is a persistence mapper, not a business layer. It does
not own business logic, authorization policy, aggregate invariants, domain event
selection, public query APIs, lifecycle hooks, hidden cascades, or broad SQL
query DSLs.
