# Read Models

Read models are query-optimized projection state. They can be stored as
document rows with a JSON/JSONB payload column, or as normalized relational
rows with table, column, key, index, relationship, and schema metadata.

The current implementation keeps these paths explicit:

| Path | API | Use when |
|---|---|---|
| Document rows | `ReadModelStore`, `.readmodel(&view)`, `ReadModelSession::document` | Whole-view JSON documents backed by a document payload column |
| Relational write mapping | `RelationalReadModel`, `ReadModelSession`, `ReadModelWritePlan` | Normalized tables, composite keys, foreign keys, JSONB columns |
| Schema lifecycle | `ReadModelSchemaRegistry`, `ReadModelSchemaAdapter` | Migration artifact generation, startup verification, explicit dev/test bootstrap |

## Document Read Models

Derive `ReadModel` on any serializable document view:

```rust
use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(collection = "game_views")]
pub struct GameView {
    #[readmodel(id)]
    pub id: String,
    pub player_name: String,
    pub score: i32,
}
```

Use `ReadModelsExt` for typed key/value CRUD:

```rust
use sourced_rust::ReadModelsExt;

let view = repo.read_models::<GameView>().get("game-42")?;
repo.read_models::<GameView>().upsert(&updated_view)?;
let read_only = repo
    .read_models::<GameView>()
    .get_by_primary_key("game-42")?;
```

This path stores one serialized model at `collection:id`. A SQL adapter can
back it with a table such as `(collection, id, version, payload jsonb)`.
Predicate helpers such as `find` and `find_one` are in-memory/document-store
helpers; SQL adapters are not required to translate Rust closures into queries.

## Relational Models

A model opts into relational metadata with `#[readmodel(table = "...")]` and
field attributes:

```rust
use serde::{Deserialize, Serialize};
use sourced_rust::ReadModel;

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "players")]
pub struct PlayerView {
    #[readmodel(id, column = "player_id")]
    pub id: String,
    pub display_name: String,
    #[readmodel(jsonb)]
    pub counters_by_game: std::collections::HashMap<String, i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "player_weapons", primary_key = ["player_id", "weapon_id"])]
pub struct PlayerWeaponView {
    #[readmodel(foreign_key = "players.player_id", delegated_from = "PlayerView.player_id")]
    pub player_id: String,
    pub weapon_id: String,
    #[readmodel(index)]
    pub acquired_at: String,
}
```

The derive emits `RelationalReadModel` metadata, row conversion, primary-key
metadata, JSONB column metadata, indexes, and an adapter-owned version column.
Composite and delegated keys are represented in the schema and in session row
mutations.

## Command-Side Atomic Writes

Use `ReadModelSession` when a command or projector stages multiple document or
normalized row mutations. The current repository APIs are synchronous:

```rust
use sourced_rust::{ReadModelSession, ReadModelSessionCommitExt};

let mut read_models = ReadModelSession::new();
read_models.save(&player)?;
read_models.save_related(&player, "weapons", &weapon)?;

repo.read_models(read_models).commit(&mut aggregate)?;
```

Async adapters can expose the same shape at their boundary:

```rust,ignore
repo.read_models(read_models).commit(&mut aggregate).await?;
```

Builder ordering is semantic staging only. These forms are equivalent:

```rust
repo.read_models(read_models)
    .outbox(message)
    .commit(&mut aggregate)?;

repo.outbox(message)
    .read_models(read_models)
    .commit(&mut aggregate)?;

repo.aggregate(&mut aggregate)
    .read_models(read_models)
    .outbox(message)
    .commit()?;
```

Document views use the same commit-builder spelling:

```rust
repo.readmodel(&board_view).commit(&mut game)?;
```

## Standalone Distributed Projectors

A read-model service can commit a session without owning an aggregate
repository:

```rust
use sourced_rust::{ReadModelError, ReadModelSession, ReadModelSessionStore};

fn project_message(
    store: &impl ReadModelSessionStore,
    event_id: &str,
    view: &GameView,
) -> Result<(), ReadModelError> {
    let mut read_models = ReadModelSession::new();
    read_models
        .document(view)?
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

## Bomberman And Document Views

Bomberman `BoardView` is intentionally a document-row read model. It stores a
whole game board view with nested players, bombs, explosions, tiles, turn state,
and counters. Do not treat it as a normalized relational ORM example.

A Postgres adapter can back this path with a JSONB payload column while
normalized relational models use real columns, primary keys, foreign keys,
indexes, and JSONB columns for selected semistructured fields.

## Queued Document Reads

`QueuedReadModelStore` preserves key/value lock behavior for document stores.
Use explicit spellings for lock intent:

```rust
let locked = store.load_for_update::<GameView>("game-42")?;
let peek = store.load_no_lock::<GameView>("game-42")?;
store.abort::<GameView>("game-42")?;
```

`get_by_primary_key` is a read helper and does not imply command-side ownership.

## Non-Goals

The relational ORM slice is a persistence mapper, not a business layer. It does
not own business logic, authorization policy, aggregate invariants, domain event
selection, public query APIs, lifecycle hooks, hidden cascades, document-store
mutation APIs, or broad SQL query DSLs.
