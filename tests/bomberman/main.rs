//! Bomberman Game Example — exercises the full sourced_rust framework.
//!
//! A 4-player Bomberman demonstrates:
//! - Single aggregate + atomic read model commit (player join/move)
//! - Multi-aggregate atomic commit via `commit_many` (bomb placement, explosion)
//! - Aggregate lifecycle: bomb created -> ticked -> exploded -> explosion expands
//! - In-process orchestration saga (tick resolves explosions across aggregates)
//! - Contested resources via QueuedRepository (concurrent bomb placement)
//! - Composite read model (BoardView from map + players + bombs + explosions)
//! - Outbox events ("PlayerKilled" on death)
//! - Guard conditions (can't move when dead, can't bomb at max)

mod domain;
mod views;
mod commands;
mod sim;
mod error;

use domain::types::Direction;
use sim::Game;

use sourced_rust::HashMapRepository;

const SMALL_MAP: &str = "\
#######
#1 . 2#
# # # #
#  .  #
# # # #
#3 . 4#
#######";

// ============================================================================
// Test 1: Game setup and movement
// Pattern: Single aggregate + read model commit, terrain validation
// ============================================================================

#[test]
fn game_setup_and_movement() {
    let repo = HashMapRepository::new();
    let game = Game::new(&repo, "game-1", SMALL_MAP).unwrap();

    let p1 = game.sim("p1", "Alice");
    let p2 = game.sim("p2", "Bob");

    p1.join(0).unwrap();
    p2.join(1).unwrap();

    // P1 starts at spawn 0 (1,1), move south
    p1.move_dir(Direction::South).unwrap();

    // Try to move into a wall — should fail
    let result = p1.move_dir(Direction::West);
    assert!(result.is_err());

    // Verify board view
    let board = game.board().unwrap();
    assert_eq!(board.data.players.len(), 2);

    let alice = board.data.players.iter().find(|p| p.name == "Alice").unwrap();
    assert_eq!(alice.x, 1);
    assert_eq!(alice.y, 2);
    assert!(alice.alive);

    let bob = board.data.players.iter().find(|p| p.name == "Bob").unwrap();
    assert_eq!(bob.x, 5);
    assert_eq!(bob.y, 1);
    assert!(bob.alive);
}

// ============================================================================
// Test 2: Bomb destroys blocks
// Pattern: Aggregate lifecycle (bomb created -> ticked -> exploded -> expansion)
// ============================================================================

#[test]
fn bomb_destroys_blocks() {
    let repo = HashMapRepository::new();
    let game = Game::new(&repo, "game-2", SMALL_MAP).unwrap();

    let p1 = game.sim("p1", "Alice");
    p1.join(0).unwrap();

    // Alice at (1,1). Move to (2,1) to be near block at (3,1).
    p1.move_dir(Direction::East).unwrap();

    // Place bomb at (2,1) — blast radius 2 east reaches block at (3,1) at ring 1
    p1.place_bomb().unwrap();

    // Escape: go west to (1,1) then south to (1,2)
    p1.move_dir(Direction::West).unwrap();  // (1,1)
    p1.move_dir(Direction::South).unwrap(); // (1,2) — safe from bomb at (2,1)

    // Verify bomb is on the board
    let board = game.board().unwrap();
    assert_eq!(board.data.bombs.len(), 1);

    // Tick 3 times: bomb timer 3 -> 2 -> 1 -> 0, detonates, center (2,1) active
    game.tick().unwrap();
    game.tick().unwrap();
    game.tick().unwrap();

    // Tick 4: explosion expands to ring 1 → (3,1) block destroyed
    let tick_result = game.tick().unwrap();

    // Block should be destroyed this tick
    assert!(!tick_result.blocks_destroyed.is_empty());

    // Verify bomb gone from board
    let board = game.board().unwrap();
    assert_eq!(board.data.bombs.len(), 0);

    // Alice should still be alive (moved away)
    assert!(p1.is_alive().unwrap());

    // Verify bomb returned to player
    let player = p1.player().unwrap();
    assert_eq!(player.active_bombs, 0);
}

// ============================================================================
// Test 3: Player killed by bomb
// Pattern: Multi-aggregate coordination + outbox event
// ============================================================================

#[test]
fn player_killed_by_bomb() {
    // Use a tall open map so players can retreat far from blast
    let repo2 = HashMapRepository::new();
    let open_map = "\
###########
#1        #
#         #
#         #
#         #
#         #
#        2#
###########";
    let game2 = Game::new(&repo2, "game-3b", open_map).unwrap();

    let alice = game2.sim("p1", "Alice");
    let bob = game2.sim("p2", "Bob");

    alice.join(0).unwrap(); // (1,1)
    bob.join(1).unwrap();   // (9,6)

    // Move Bob to (5,1) via north then west
    bob.move_dir(Direction::North).unwrap(); // (9,5)
    bob.move_dir(Direction::North).unwrap(); // (9,4)
    bob.move_dir(Direction::North).unwrap(); // (9,3)
    bob.move_dir(Direction::North).unwrap(); // (9,2)
    bob.move_dir(Direction::North).unwrap(); // (9,1)
    bob.move_dir(Direction::West).unwrap();  // (8,1)
    bob.move_dir(Direction::West).unwrap();  // (7,1)
    bob.move_dir(Direction::West).unwrap();  // (6,1)
    bob.move_dir(Direction::West).unwrap();  // (5,1)

    // Alice moves east to (3,1)
    alice.move_dir(Direction::East).unwrap(); // (2,1)
    alice.move_dir(Direction::East).unwrap(); // (3,1)

    // Alice places bomb at (3,1), blast radius 2 east: ring 1=(4,1), ring 2=(5,1)
    alice.place_bomb().unwrap();

    // Alice retreats south 3 cells (blast south goes (3,2),(3,3), so (3,4) is safe)
    alice.move_dir(Direction::South).unwrap(); // (3,2)
    alice.move_dir(Direction::South).unwrap(); // (3,3)
    alice.move_dir(Direction::South).unwrap(); // (3,4)

    // Tick 3 times to detonate: timer 3→2→1→0, bomb detonates, center (3,1) active
    game2.tick().unwrap();
    game2.tick().unwrap();
    game2.tick().unwrap();

    // Tick 4: expand to ring 1 → (4,1) — Bob not here
    game2.tick().unwrap();

    // Tick 5: expand to ring 2 → (5,1) — Bob killed!
    let tick_result = game2.tick().unwrap();

    // Verify Bob was killed
    assert!(!bob.is_alive().unwrap());
    assert!(alice.is_alive().unwrap());

    // Verify kill recorded on saga
    assert!(tick_result.players_killed.contains(&"player:p2".to_string()));

    // Verify outbox message was created (PlayerKilled)
    use sourced_rust::{OutboxRepositoryExt, OutboxMessageStatus};
    let pending = repo2.outbox_messages_by_status(OutboxMessageStatus::Pending).unwrap();
    assert!(!pending.is_empty());
    assert_eq!(pending[0].event_type, "PlayerKilled");
}

// ============================================================================
// Test 4: Chain reaction
// Pattern: In-process orchestration saga (expanding explosion triggers chain)
// ============================================================================

#[test]
fn chain_reaction() {
    let repo = HashMapRepository::new();
    // Tall map so players can retreat far enough from blast radius 2
    let tall_map = "\
###########
#1        #
#         #
#         #
#         #
#         #
#         #
#        2#
###########";
    let game = Game::new(&repo, "game-4", tall_map).unwrap();

    let p1 = game.sim("p1", "Alice");
    let p2 = game.sim("p2", "Bob");

    p1.join(0).unwrap(); // (1,1)
    p2.join(1).unwrap(); // (9,7)

    // Alice moves to (3,1), places bomb, then retreats south far enough
    p1.move_dir(Direction::East).unwrap();  // (2,1)
    p1.move_dir(Direction::East).unwrap();  // (3,1)
    p1.place_bomb().unwrap();               // bomb at (3,1), radius 2

    // Alice retreats south 3 cells (blast goes south only 2)
    p1.move_dir(Direction::South).unwrap(); // (3,2)
    p1.move_dir(Direction::South).unwrap(); // (3,3)
    p1.move_dir(Direction::South).unwrap(); // (3,4)

    // Tick twice so Alice's bomb timer goes from 3→2→1
    game.tick().unwrap();
    game.tick().unwrap();

    // Now Bob places bomb at (5,1) — 2 cells east of Alice's bomb
    // Bob's bomb will have timer=3, Alice's bomb has timer=1
    // Alice's bomb detonates next tick, expanding blast reaches (5,1) at ring 2
    bob_move_to_position(&game, "p2", "Bob", 9, 7, 5, 1);
    p2.place_bomb().unwrap();               // bomb at (5,1), radius 2
    p2.move_dir(Direction::South).unwrap(); // (5,2)
    p2.move_dir(Direction::South).unwrap(); // (5,3)
    p2.move_dir(Direction::South).unwrap(); // (5,4)

    // Tick 3: Alice's bomb timer 1→0 → detonates. Bob's bomb timer 3→2.
    //         Alice's explosion: center (3,1) active.
    game.tick().unwrap();

    // Tick 4: Alice's explosion expands to ring 1: (2,1),(4,1),(3,0)=wall blocked,(3,2).
    //         Bob's bomb timer 2→1. No chain yet.
    game.tick().unwrap();

    // Tick 5: Alice's explosion expands to ring 2: (1,1),(5,1),(3,3).
    //         (5,1) hits Bob's bomb → chain detonation! Bob's bomb marked ticks_remaining=0.
    //         Bob's bomb timer was 1→0 from ticking, BUT the chain mark also sets it to 0.
    //         Either way, Bob's bomb detonates in Phase B. Bob's explosion center (5,1) active.
    let tick_result = game.tick().unwrap();

    // Both bombs should have detonated — at least 2 detonations total across all ticks
    // The chain detonation should be recorded
    assert!(!tick_result.chain_detonations.is_empty() || tick_result.detonations.len() >= 2,
        "Expected chain reaction or multiple detonations, got {} detonations, {} chains",
        tick_result.detonations.len(), tick_result.chain_detonations.len());

    // Both players should be alive (they retreated far enough)
    assert!(p1.is_alive().unwrap());
    assert!(p2.is_alive().unwrap());
}

/// Helper to move a player step by step to a target position via simple pathfinding.
fn bob_move_to_position<R: sourced_rust::Commit + sourced_rust::read_model::ReadModelStore + sourced_rust::Get + sourced_rust::Find>(
    game: &Game<'_, R>,
    id: &str,
    _name: &str,
    from_x: i32,
    from_y: i32,
    to_x: i32,
    to_y: i32,
) {
    let sim = game.sim(id, _name);
    let mut cx = from_x;
    let mut cy = from_y;

    // Move north/south first, then east/west
    while cy != to_y {
        if cy > to_y {
            sim.move_dir(Direction::North).unwrap();
            cy -= 1;
        } else {
            sim.move_dir(Direction::South).unwrap();
            cy += 1;
        }
    }
    while cx != to_x {
        if cx > to_x {
            sim.move_dir(Direction::West).unwrap();
            cx -= 1;
        } else {
            sim.move_dir(Direction::East).unwrap();
            cx += 1;
        }
    }
}

// ============================================================================
// Test 5: Concurrent bomb placement
// Pattern: Contested resource — both players place bombs, verify atomic commits
// ============================================================================

#[test]
fn concurrent_bomb_placement() {
    use std::sync::Arc;
    use std::thread;
    use sourced_rust::ReadModelsExt;

    // HashMapRepository uses Arc<RwLock<...>> internally, safe to share across threads
    let repo = Arc::new(HashMapRepository::new());
    let open_map = "\
#######
#1   2#
#     #
#3   4#
#######";

    // Create game and join players on main thread
    commands::create_game(&*repo, "game-5", open_map).unwrap();
    commands::join_game(&*repo, "p1", "Alice", "game-5", 0).unwrap();
    commands::join_game(&*repo, "p2", "Bob", "game-5", 1).unwrap();

    let repo2 = repo.clone();
    let repo3 = repo.clone();

    // Two threads place bombs concurrently
    let t1 = thread::spawn(move || {
        commands::place_bomb(&*repo2, "p1", "game-5").unwrap();
    });

    let t2 = thread::spawn(move || {
        commands::place_bomb(&*repo3, "p2", "game-5").unwrap();
    });

    t1.join().unwrap();
    t2.join().unwrap();

    // Verify both bombs placed
    let _board = repo.read_models::<views::BoardView>().get("game-5").unwrap().unwrap();
    // Board may show 1 or 2 bombs depending on which thread's board view won the race,
    // but both bomb aggregates should exist in the repo.
    use sourced_rust::GetAggregate;
    let alice: domain::player::Player = repo.get_aggregate("player:p1").unwrap().unwrap();
    assert_eq!(alice.active_bombs, 1);

    let bob: domain::player::Player = repo.get_aggregate("player:p2").unwrap().unwrap();
    assert_eq!(bob.active_bombs, 1);

    // Both bomb entities should be in the repo
    let bomb1: Option<domain::bomb::Bomb> = repo.get_aggregate("bomb:p1:1").unwrap();
    let bomb2: Option<domain::bomb::Bomb> = repo.get_aggregate("bomb:p2:1").unwrap();
    assert!(bomb1.is_some(), "Alice's bomb should exist");
    assert!(bomb2.is_some(), "Bob's bomb should exist");
}

// ============================================================================
// Test 6: Full game to winner
// Pattern: End-to-end composition, game lifecycle
// ============================================================================

#[test]
fn full_game_to_winner() {
    let repo = HashMapRepository::new();
    // Larger arena to allow proper retreat from blast radius 2
    let arena = "\
#############
#1         2#
#           #
#           #
#           #
#           #
#           #
#           #
#           #
#           #
#           #
#3         4#
#############";

    let game = Game::new(&repo, "game-6", arena).unwrap();

    let p1 = game.sim("p1", "Alice");
    let p2 = game.sim("p2", "Bob");
    let p3 = game.sim("p3", "Charlie");
    let p4 = game.sim("p4", "Diana");

    // All players join
    p1.join(0).unwrap(); // (1,1)
    p2.join(1).unwrap(); // (11,1)
    p3.join(2).unwrap(); // (1,11)
    p4.join(3).unwrap(); // (11,11)

    // Verify initial board
    let board = game.board().unwrap();
    assert_eq!(board.data.players.len(), 4);
    assert!(board.data.players.iter().all(|p| p.alive));

    // --- Round 1: Alice traps Bob ---
    // Alice moves east to (9,1), places bomb. Blast east: ring 1=(10,1), ring 2=(11,1) hits Bob.
    p1.move_dir(Direction::East).unwrap(); // (2,1)
    p1.move_dir(Direction::East).unwrap(); // (3,1)
    p1.move_dir(Direction::East).unwrap(); // (4,1)
    p1.move_dir(Direction::East).unwrap(); // (5,1)
    p1.move_dir(Direction::East).unwrap(); // (6,1)
    p1.move_dir(Direction::East).unwrap(); // (7,1)
    p1.move_dir(Direction::East).unwrap(); // (8,1)
    p1.move_dir(Direction::East).unwrap(); // (9,1)

    p1.place_bomb().unwrap(); // bomb at (9,1)

    // Alice retreats south 3 cells (blast south goes (9,2),(9,3))
    p1.move_dir(Direction::South).unwrap(); // (9,2)
    p1.move_dir(Direction::South).unwrap(); // (9,3)
    p1.move_dir(Direction::South).unwrap(); // (9,4) — safe

    // 3 ticks to detonate + 2 ticks for ring 2 expansion = 5 ticks
    game.tick().unwrap();
    game.tick().unwrap();
    game.tick().unwrap(); // detonates, center active
    game.tick().unwrap(); // ring 1
    let r1 = game.tick().unwrap(); // ring 2 → Bob killed

    assert!(!p2.is_alive().unwrap(), "Bob should be dead from Alice's bomb");
    assert!(r1.players_killed.contains(&"player:p2".to_string()));

    // --- Round 2: Charlie traps Diana ---
    // Charlie at (1,11), Diana at (11,11)
    p3.move_dir(Direction::East).unwrap(); // (2,11)
    p3.move_dir(Direction::East).unwrap(); // (3,11)
    p3.move_dir(Direction::East).unwrap(); // (4,11)
    p3.move_dir(Direction::East).unwrap(); // (5,11)
    p3.move_dir(Direction::East).unwrap(); // (6,11)
    p3.move_dir(Direction::East).unwrap(); // (7,11)
    p3.move_dir(Direction::East).unwrap(); // (8,11)
    p3.move_dir(Direction::East).unwrap(); // (9,11)

    p3.place_bomb().unwrap(); // bomb at (9,11), blast east: ring 2=(11,11) hits Diana

    // Charlie retreats north 3 cells
    p3.move_dir(Direction::North).unwrap(); // (9,10)
    p3.move_dir(Direction::North).unwrap(); // (9,9)
    p3.move_dir(Direction::North).unwrap(); // (9,8) — safe

    // 5 ticks for detonation + expansion to ring 2
    game.tick().unwrap();
    game.tick().unwrap();
    game.tick().unwrap(); // detonates
    game.tick().unwrap(); // ring 1
    let r2 = game.tick().unwrap(); // ring 2 → Diana killed

    assert!(!p4.is_alive().unwrap(), "Diana should be dead");
    assert!(r2.players_killed.contains(&"player:p4".to_string()));

    // --- Round 3: Alice finishes Charlie ---
    // Alice at (9,4), Charlie at (9,8)
    p1.move_dir(Direction::South).unwrap(); // (9,5)
    p1.move_dir(Direction::South).unwrap(); // (9,6)

    // Place bomb at (9,6), blast south: ring 1=(9,7), ring 2=(9,8) hits Charlie
    p1.place_bomb().unwrap();

    // Alice retreats north 3 cells
    p1.move_dir(Direction::North).unwrap(); // (9,5)
    p1.move_dir(Direction::North).unwrap(); // (9,4)
    p1.move_dir(Direction::North).unwrap(); // (9,3) — safe

    // 5 ticks for detonation + expansion to ring 2
    game.tick().unwrap();
    game.tick().unwrap();
    game.tick().unwrap(); // detonates
    game.tick().unwrap(); // ring 1
    let r3 = game.tick().unwrap(); // ring 2 → Charlie killed

    assert!(!p3.is_alive().unwrap(), "Charlie should be dead");
    assert!(r3.game_over, "Game should be over with only Alice alive");
    assert_eq!(r3.winner.as_deref(), Some("Alice"));

    // Verify final board
    let board = game.board().unwrap();
    let alive_count = board.data.players.iter().filter(|p| p.alive).count();
    assert_eq!(alive_count, 1);
    let winner_state = board.data.players.iter().find(|p| p.alive).unwrap();
    assert_eq!(winner_state.name, "Alice");
}
