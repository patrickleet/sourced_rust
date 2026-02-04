# Projections and Commit Builder

## Problem Statement

Currently, the library supports committing multiple entities of the same aggregate type:
```rust
repo.commit_all(&mut [&mut todo1, &mut todo2])?;
```

And committing an aggregate with an outbox message:
```rust
repo.outbox(&mut message).commit(&mut aggregate)?;
```

**What's missing:**
1. Read models / projections (non-event-sourced views optimized for queries)
2. A general way to commit heterogeneous things together atomically

## Proposed Solution

Extend the `.outbox()` chainable pattern to support projections:

```rust
repo
    .projection(&mut game_view)
    .projection(&mut player_stats)
    .outbox(&mut message)
    .commit(&mut game)?;
```

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                      CommitBuilder                              │
│                                                                 │
│   .projection(&mut game_view)     ──┐                           │
│   .projection(&mut player_stats)  ──┼── accumulates entities    │
│   .outbox(&mut message)           ──┘                           │
│   .commit(&mut game)              ──── adds primary + executes  │
│                                                                 │
│                         │                                       │
│                         ▼                                       │
│              repo.commit(&mut [all entities])                   │
└─────────────────────────────────────────────────────────────────┘
```

### Key Insight

Projections wrap an `Entity` internally, just like `OutboxMessage` does. This means:
- Storage remains uniform (everything is entities)
- The existing `commit()` infrastructure works unchanged
- Projections serialize their current state as a snapshot event

---

## API Design

### Starting the Chain

```rust
// Start with projection
repo.projection(&mut view).commit(&mut aggregate)?;

// Start with outbox (existing)
repo.outbox(&mut msg).commit(&mut aggregate)?;

// Chain multiple
repo
    .projection(&mut view1)
    .projection(&mut view2)
    .outbox(&mut msg)
    .commit(&mut aggregate)?;

// Just commit (existing, unchanged)
repo.commit(&mut aggregate)?;
```

### Projection Definition

```rust
#[derive(Serialize, Deserialize)]
pub struct BlobGameView {
    pub id: String,
    pub address: String,
    pub score: u32,
    pub status: GameStatus,
    // ... optimized for API consumption
}

impl ProjectionSchema for BlobGameView {
    fn key(&self) -> String {
        format!("game:{}", self.id)
    }
}
```

### Usage Example (BlobGame)

```rust
fn handle_move(
    repo: &impl Repository,
    game_id: &str,
    direction: Direction,
    time: u64,
) -> Result<BlobGameView, Error> {
    // 1. Load aggregate
    let mut game: BlobGame = repo.get_aggregate(game_id)?.ok_or(NotFound)?;

    // 2. Load or create projections
    let mut game_view = repo.get_projection::<BlobGameView>(&format!("game:{}", game_id))?
        .unwrap_or_else(|| Projection::new(BlobGameView::default()));

    let mut player_index = repo.get_projection::<PlayerGamesIndex>(&format!("player:{}", game.address()))?
        .unwrap_or_else(|| Projection::new(PlayerGamesIndex::new(game.address())));

    // 3. Execute command
    match direction {
        Direction::Up => game.up(Some(time)),
        Direction::Down => game.down(Some(time)),
        Direction::Left => game.left(Some(time)),
        Direction::Right => game.right(Some(time)),
    }

    // 4. Update projections from aggregate state
    *game_view.data_mut() = BlobGameView::from_aggregate(&game, time);
    player_index.data_mut().update_from_game(&game);

    // 5. Create outbox message
    let mut outbox = OutboxMessage::encode(
        format!("{}:v{}", game_id, game.entity.version()),
        "GameUpdated",
        game_view.data(),
    )?;

    // 6. Commit everything atomically
    repo
        .projection(&mut game_view)
        .projection(&mut player_index)
        .outbox(&mut outbox)
        .commit(&mut game)?;

    Ok(game_view.into_data())
}
```

---

## Implementation Details

### 1. `Projection<T>` Wrapper

```rust
/// A projection wraps typed data with an Entity for storage.
/// Unlike aggregates, projections overwrite state rather than append events.
pub struct Projection<T> {
    entity: Entity,
    data: T,
    dirty: bool,
}

impl<T: ProjectionSchema> Projection<T> {
    /// Create a new projection with initial data
    pub fn new(data: T) -> Self {
        let key = data.key();
        Self {
            entity: Entity::with_id(&key),
            data,
            dirty: true,
        }
    }

    /// Load from an existing entity (used by repository)
    pub fn from_entity(entity: Entity) -> Result<Self, ProjectionError>
    where
        T: DeserializeOwned,
    {
        // Get the latest snapshot event
        let snapshot = entity.events().last()
            .ok_or(ProjectionError::NoSnapshot)?;
        let data: T = bitcode::deserialize(snapshot.payload())?;
        Ok(Self { entity, data, dirty: false })
    }

    pub fn data(&self) -> &T { &self.data }

    pub fn data_mut(&mut self) -> &mut T {
        self.dirty = true;
        &mut self.data
    }

    pub fn into_data(self) -> T { self.data }

    /// Sync data to entity (called before commit)
    pub(crate) fn sync(&mut self) {
        if self.dirty {
            self.entity.set_snapshot(&self.data);
            self.dirty = false;
        }
    }

    pub(crate) fn entity_mut(&mut self) -> &mut Entity {
        &mut self.entity
    }
}
```

### 2. `ProjectionSchema` Trait

```rust
/// Trait for types that can be stored as projections.
pub trait ProjectionSchema: Serialize + DeserializeOwned {
    /// Returns the storage key for this projection instance.
    /// Convention: use prefixes like "game:", "player:", "leaderboard:"
    fn key(&self) -> String;
}
```

### 3. `CommitBuilder`

```rust
/// Builder for chaining multiple items into a single atomic commit.
pub struct CommitBuilder<'a, R> {
    repo: &'a R,
    entities: Vec<&'a mut Entity>,
}

impl<'a, R> CommitBuilder<'a, R> {
    pub(crate) fn new(repo: &'a R) -> Self {
        Self { repo, entities: vec![] }
    }

    /// Add a projection to the commit
    pub fn projection<T: ProjectionSchema>(mut self, proj: &'a mut Projection<T>) -> Self {
        proj.sync();
        self.entities.push(proj.entity_mut());
        self
    }

    /// Add an outbox message to the commit
    pub fn outbox(mut self, msg: &'a mut OutboxMessage) -> Self {
        self.entities.push(msg.entity_mut());
        self
    }

    /// Commit all accumulated items plus the primary aggregate
    pub fn commit<A: Aggregate>(mut self, aggregate: &'a mut A) -> Result<(), RepositoryError>
    where
        R: Commit,
    {
        self.entities.push(aggregate.entity_mut());
        self.repo.commit(&mut self.entities[..])
    }

    /// Commit without a primary aggregate (projections/outbox only)
    pub fn commit_all(self) -> Result<(), RepositoryError>
    where
        R: Commit,
    {
        self.repo.commit(&mut self.entities[..])
    }
}
```

### 4. Extension Trait

```rust
/// Extension trait to start a commit builder chain with a projection.
pub trait ProjectionCommitExt: Commit + Sized {
    fn projection<'a, T: ProjectionSchema>(
        &'a self,
        proj: &'a mut Projection<T>,
    ) -> CommitBuilder<'a, Self> {
        CommitBuilder::new(self).projection(proj)
    }
}

impl<R: Commit> ProjectionCommitExt for R {}
```

### 5. Repository Extension for Loading Projections

```rust
/// Extension trait for loading projections from a repository.
pub trait ProjectionRepository: Get {
    fn get_projection<T: ProjectionSchema>(&self, key: &str) -> Result<Option<Projection<T>>, RepositoryError> {
        let entity = self.get(key)?;
        match entity {
            Some(e) => Ok(Some(Projection::from_entity(e)?)),
            None => Ok(None),
        }
    }
}

impl<R: Get> ProjectionRepository for R {}
```

### 6. Entity Enhancement

Add a method to Entity for setting snapshot data:

```rust
impl Entity {
    /// Replace all events with a single snapshot event.
    /// Used by projections to store current state.
    pub fn set_snapshot<T: Serialize>(&mut self, data: &T) {
        let payload = bitcode::serialize(data).expect("serialization failed");
        // Clear existing events and add snapshot
        self.events.clear();
        self.events.push(EventRecord::new("Snapshot", payload, 1));
        self.version = 1;
    }
}
```

**Alternative (append-only):**
```rust
impl Entity {
    /// Append a snapshot event with current state.
    /// Loading takes the latest snapshot.
    pub fn append_snapshot<T: Serialize>(&mut self, data: &T) {
        let payload = bitcode::serialize(data).expect("serialization failed");
        self.digest_with_payload("Snapshot", payload);
    }
}
```

---

## File Structure

```
src/
├── projection/
│   ├── mod.rs           # Projection<T>, ProjectionSchema, ProjectionError
│   ├── commit.rs        # CommitBuilder, ProjectionCommitExt
│   └── repository.rs    # ProjectionRepository trait
├── core/
│   └── entity.rs        # Add set_snapshot() method
└── lib.rs               # Re-exports

tests/
├── projections.rs       # Projection tests
└── support/
    └── blob_game/
        ├── mod.rs
        ├── aggregate.rs # (existing)
        └── views.rs     # BlobGameView, PlayerGamesIndex projections
```

---

## Implementation Order

| Phase | Task | Files |
|-------|------|-------|
| 1 | Add `set_snapshot()` to Entity | `src/core/entity.rs` |
| 2 | Create Projection module | `src/projection/mod.rs` |
| 3 | Create CommitBuilder | `src/projection/commit.rs` |
| 4 | Add ProjectionRepository | `src/projection/repository.rs` |
| 5 | Re-exports in lib.rs | `src/lib.rs` |
| 6 | Create BlobGame projections | `tests/support/blob_game/views.rs` |
| 7 | Integration tests | `tests/projections.rs` |
| 8 | Update existing OutboxCommit to use CommitBuilder (optional) | `src/outbox/commit.rs` |

---

## Open Questions

### 1. Snapshot storage: Replace vs Append?

**Replace (recommended for projections):**
- Cleaner storage (one event per projection)
- Requires adding `set_snapshot()` that clears events

**Append:**
- Simpler (uses existing `digest`)
- Accumulates events over time (wasteful)
- Would need compaction strategy later

**Recommendation:** Replace for projections, since they represent current state, not history.

### 2. Key conventions

Should we enforce key prefixes?

```rust
// Option A: Free-form keys
impl ProjectionSchema for BlobGameView {
    fn key(&self) -> String { format!("game:{}", self.id) }
}

// Option B: Enforce prefix via trait
pub trait ProjectionSchema {
    const PREFIX: &'static str;
    fn id(&self) -> &str;
    fn key(&self) -> String { format!("{}:{}", Self::PREFIX, self.id()) }
}
```

**Recommendation:** Option A (free-form) for flexibility, with convention documented.

### 3. Typed repository access

Should projections have their own typed repository wrapper (like `AggregateRepository`)?

```rust
// Option A: Extension trait only
repo.get_projection::<BlobGameView>("game:123")?;

// Option B: Typed wrapper
let views: ProjectionRepository<BlobGameView> = repo.projections();
views.get("123")?;  // Prefix added automatically
```

**Recommendation:** Start with Option A, add Option B later if needed.

---

## Comparison: Event-Sourced vs Projection

| Aspect | Event-Sourced Aggregate | Projection |
|--------|------------------------|------------|
| Purpose | Domain logic, source of truth | Query optimization |
| Storage | Append event history | Overwrite current state |
| Loading | Replay events → state | Deserialize latest snapshot |
| Schema | Domain-driven | Query-driven |
| Example | `BlobGame` | `BlobGameView` |

---

## Future Enhancements

1. **Projection versioning**: Track schema versions for migrations
2. **Async projections**: Update projections via outbox workers
3. **Projection rebuild**: Replay aggregate events to rebuild projections
4. **Typed projection repository**: `repo.projections::<BlobGameView>()`
5. **Projection indexes**: Secondary indexes for efficient queries
