# Blob Game remake — e2e-ui CQRS/ES demo

Remake of **internet-game-meta** `projects/ig-blob-game-model-service` (and Blob UI) as a domain + GraphQL surface + Svelte page inside `tests/e2e-ui`. Goal: show **aggregates emit events → projectors update read models → UI queries/subscribes**.

## Sources

| Legacy | Role |
|--------|------|
| `ig-blob-game-model-service/src/models/BlobGame.ts` | Aggregate rules |
| `…/lib/constants.ts` | Tile enum |
| `…/lib/levels.ts` | Level generation (reference only) |
| `…/handlers/blob-game.*.ts` | Commands: initialize, start-next-level, move |
| `internet-game/.../Games/Blob/*` | UI reference only |
| `distributed/tests/blob_game/` | Earlier Rust sketch (tile death values **not** used; see below) |

## Tile semantics (canonical — match JS)

| Value | Name | Meaning |
|------:|------|---------|
| `9` | `player` | Current cell |
| `0` | `hole` | Instant death if entered |
| `1` | `unvisited` | Scoring cell; become `visited` after leave |
| `2` | `visited` | Re-entry → death (`dead_by_suicide`) |
| `3` | `dead_by_suicide` | Died by revisit / timeout |
| `4` | `dead_by_hole` | Died by hole |

> Earlier Rust port used `10`/`11` for death tiles. **This remake uses `3`/`4` like JS.**

## Domain rules

1. **Initialize** — set `game_id`, `owner_id` (auth user). Level list empty; `current_level = 0`; `current_level_completed = true` (so first level can start).
2. **Start next level** — only if initialized and `current_level_completed` and not dead. Append map; `current_level += 1`; `current_level_completed = false`. Player must appear exactly once (`tile = 9`).
3. **Move** (up/down/left/right) — only if a level is active and player not dead.
   - Edge moves **reject** (no event, no mutation).
   - Leave previous cell as `visited` (`2`).
   - Enter `hole` → cell becomes `dead_by_hole` (`4`), `player_dead = true`.
   - Enter `visited` → cell becomes `dead_by_suicide` (`3`), `player_dead = true`.
   - Enter `unvisited` → cell becomes `player` (`9`), **score += 1**.
4. **Level complete** — after a move, if no `unvisited` remains and player not dead: `current_level_completed = true`, level.completed = true.
5. **Timed mode** — optional; not required for e2e demo (omit or default off).

## Commands (GraphQL mutations)

| Command | GraphQL field | Input | Notes |
|---------|---------------|-------|--------|
| `blob.start` | `blob_games_start` | `game_id` | Auth owner; creates game + starts **demo level 1** (fixed small map) |
| `blob.move` | `blob_games_move` | `game_id`, `direction` | `up` \| `down` \| `left` \| `right` |
| `blob.start_level` | `blob_games_start_level` | `game_id`, optional map | Next level when previous complete; default demo map if omitted |

Owner is **always** the session principal (`x-user-id`); never from client for ownership.

## Events (domain → bus → projector)

Each successful mutation records a full **fact** snapshot (same pattern as `TodoFact`):

| Event name | When |
|------------|------|
| `blob.initialized` | After start (init only if split; usually folded into start) |
| `blob.level_started` | Level appended / active |
| `blob.moved` | After a successful move (score/death/complete already applied) |

**Payload (`BlobGameFact`):**

```text
game_id, owner_id, score, player_dead, current_level,
current_level_completed, map_json (stringified number[][]),
status: "active" | "dead" | "level_complete"
```

**Commands upsert `blob_games` after commit** so the mutation response is the live RM row.
**Client stays optimistic:** paint with local rules first, then reconcile with the command
payload (authoritative RM). Projectors remain idempotent backup — UI does not wait on them.

## Read model

**Table:** `blob_games`

| Column | Type | Notes |
|--------|------|--------|
| `game_id` | text PK | Aggregate id |
| `owner_id` | text | RLS: user sees own rows |
| `score` | integer | |
| `player_dead` | bool / int | |
| `current_level` | integer | |
| `current_level_completed` | bool | |
| `map_json` | text | JSON 2D array of tile ints |
| `status` | text | `active` / `dead` / `level_complete` |

GraphQL: query `blob_games` (filter by owner for role `user`); optional subscription on same table via ChangeHub.

## UI page `/blob`

1. Auth required (same as todos).
2. **New game** → `blob_games_start` with client-generated `game_id`.
3. Render grid from `map_json` (colors by tile value).
4. Arrow buttons / keyboard → `blob_games_move`.
5. Show score, dead banner, level-complete + optional **Next level**.
6. Live list/query store like todos (document store + command pipeline).

## Differences from legacy JS service

| Legacy | e2e remake |
|--------|------------|
| sourced JS Entity + Knative bus | Distributed Rust aggregate + outbox bus |
| Hasura session `x-hasura-user-id` | GraphQL OIDC / DevHeaders `x-user-id` |
| Separate denormalizer HTTP | In-process projector |
| 12×12 random holes (heuristic) | **8×8 generated** maps with hole spacing + **Hamiltonian passability** check |
| minigameId / web3 address | `owner_id` = auth user |
| Timed 5-minute death | Not required for demo |

## Level generation

- `blob_domain::generate_level(level_index)` builds a **6×6** grid, player at `(0,0)`,
  places holes with spacing heuristics (inspired by original `basicHoleValidation`),
  then accepts only maps that are **Hamiltonian-passable** (some path visits every
  non-hole cell exactly once). Falls back to a no-hole grid if generation fails.
- Each **New game** / **Next level** draws a new map; hole count grows slowly with level.
- Unit tests use fixed maps (`test_map_*`, `demo_map`) for deterministic rules tests.

## Test plan (in-repo)

1. **Domain unit** — initialize/start → move score; hole death; revisit death; level complete; edge reject.
2. **Projector unit** — apply `blob.moved` fact → read-model row matches.
3. **Live GraphQL** — `blob_games_start` + `blob_games_move` + query `blob_games` with DevHeaders; assert score/map change.
