//! Domain-event projections for Blob query models.

use distributed::domain_event::{DomainEventBodyContract, DomainEventContract};
use distributed::projection;
use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed::DomainEventDescriptor;

use crate::{BlobGameState, BlobGames};

macro_rules! state_event_contract {
    ($name:ident, $event:literal) => {
        pub enum $name {}

        impl DomainEventContract for $name {
            const EVENT_NAME: &'static str = $event;
            const EVENT_VERSION: u64 = 1;

            fn descriptor() -> DomainEventDescriptor {
                DomainEventDescriptor::state::<BlobGameState>($event, 1)
            }
        }

        impl DomainEventBodyContract<BlobGameState> for $name {}
    };
}

state_event_contract!(BlobInitializedDomainEvent, "blob.initialized");
state_event_contract!(BlobLevelStartedDomainEvent, "blob.level_started");
state_event_contract!(BlobStartedDomainEvent, "blob.started");
state_event_contract!(BlobMovedDomainEvent, "blob.moved");

/// One complete state upsert for every direct Blob transition.
pub const BLOB_GAMES: ProjectionDescriptor<DirectCandidate> = projection! {
    name: "project_blob";
    version: 1;
    epoch: "e2e-ui-blob-v2";
    partition: unit;

    on [
        "blob.initialized",
        "blob.level_started",
        "blob.started",
        "blob.moved"
    ] version 1 (state: BlobGameState) {
        upsert BlobGames from state as game;
    }
};

/// Compile-time guards for the current one-row direct projection boundary.
///
/// A patch descriptor reaches the fluent commit builder but has no
/// `projected` terminal.
///
/// ```compile_fail,E0599
/// use blob_domain::{BlobGame, BlobGameState, BlobGames};
/// use distributed::{
///     microsvc::{AggregateCheckout, CausalCommandContext},
///     projection,
///     projection::lower::{EventualOnly, ProjectionDescriptor},
/// };
///
/// const PATCH: ProjectionDescriptor<EventualOnly> = projection! {
///     name: "blob_patch_fixture";
///     version: 1;
///     epoch: "fixture";
///     partition: unit;
///     on "blob.moved" version 1 (state: BlobGameState) {
///         patch BlobGames {
///             key { game_id: state.game_id },
///             set { status: state.status }
///         };
///     }
/// };
///
/// fn cannot_claim_projected(
///     ctx: &CausalCommandContext<'_, BlobGame>,
///     game: AggregateCheckout<BlobGame>,
///     view: BlobGames,
/// ) {
///     let _ = ctx.project(PATCH).commit(game).unwrap().projected(view);
/// }
/// ```
///
/// A delete descriptor is also excluded from `Projected<T>`.
///
/// ```compile_fail,E0599
/// use blob_domain::{BlobGame, BlobGameState, BlobGames};
/// use distributed::{
///     microsvc::{AggregateCheckout, CausalCommandContext},
///     projection,
///     projection::lower::{EventualOnly, ProjectionDescriptor},
/// };
///
/// const DELETE: ProjectionDescriptor<EventualOnly> = projection! {
///     name: "blob_delete_fixture";
///     version: 1;
///     epoch: "fixture";
///     partition: unit;
///     on "blob.moved" version 1 (state: BlobGameState) {
///         delete BlobGames { key { game_id: state.game_id } };
///     }
/// };
///
/// fn cannot_claim_projected(
///     ctx: &CausalCommandContext<'_, BlobGame>,
///     game: AggregateCheckout<BlobGame>,
///     view: BlobGames,
/// ) {
///     let _ = ctx.project(DELETE).commit(game).unwrap().projected(view);
/// }
/// ```
///
/// More than one row operation cannot claim the single-row terminal.
///
/// ```compile_fail,E0599
/// use blob_domain::{BlobGame, BlobGameState, BlobGames};
/// use distributed::{
///     microsvc::{AggregateCheckout, CausalCommandContext},
///     projection,
///     projection::lower::{EventualOnly, ProjectionDescriptor},
///     ReadModel,
/// };
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Clone, Serialize, Deserialize, ReadModel)]
/// #[readmodel(primary_key = ["game_id"])]
/// struct BlobGameAudits {
///     #[readmodel(id)]
///     game_id: String,
///     owner_id: String,
///     score: i64,
///     player_dead: bool,
///     current_level: i64,
///     current_level_completed: bool,
///     map_json: String,
///     status: String,
/// }
///
/// const MULTI_ROW: ProjectionDescriptor<EventualOnly> = projection! {
///     name: "blob_multi_row_fixture";
///     version: 1;
///     epoch: "fixture";
///     partition: unit;
///     on "blob.moved" version 1 (state: BlobGameState) {
///         upsert BlobGames from state as first;
///         upsert BlobGameAudits from state as second;
///     }
/// };
///
/// fn cannot_claim_projected(
///     ctx: &CausalCommandContext<'_, BlobGame>,
///     game: AggregateCheckout<BlobGame>,
///     view: BlobGames,
/// ) {
///     let _ = ctx.project(MULTI_ROW).commit(game).unwrap().projected(view);
/// }
/// ```
#[doc(hidden)]
pub struct BlobDirectEligibilityGuards;

#[cfg(test)]
mod tests {
    use distributed::projection::lower::{EventualOnly, ProjectionDescriptor};
    use distributed::{
        projection, ProjectionMutationKind, RelationalReadModel, RowValue, TableMutation,
    };
    use serde::{Deserialize, Serialize};

    use super::*;
    use crate::{test_map_no_holes, BlobGame, Direction};

    #[derive(Clone, Serialize, Deserialize, distributed::ReadModel)]
    #[readmodel(primary_key = ["game_id"])]
    struct BlobGameAudits {
        #[readmodel(id)]
        game_id: String,
        owner_id: String,
        score: i64,
        player_dead: bool,
        current_level: i64,
        current_level_completed: bool,
        map_json: String,
        status: String,
    }

    const PATCH_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = projection! {
        name: "blob_patch_fixture";
        version: 1;
        epoch: "fixture";
        partition: unit;

        on "blob.moved" version 1 (state: BlobGameState) {
            patch BlobGames {
                key { game_id: state.game_id },
                set { status: state.status }
            };
        }
    };

    const DELETE_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = projection! {
        name: "blob_delete_fixture";
        version: 1;
        epoch: "fixture";
        partition: unit;

        on "blob.moved" version 1 (state: BlobGameState) {
            delete BlobGames { key { game_id: state.game_id } };
        }
    };

    const MULTI_ROW_BLOB_GAMES: ProjectionDescriptor<EventualOnly> = projection! {
        name: "blob_multi_row_fixture";
        version: 1;
        epoch: "fixture";
        partition: unit;

        on "blob.moved" version 1 (state: BlobGameState) {
            upsert BlobGames from state as first;
            upsert BlobGameAudits from state as second;
        }
    };

    fn assert_eventual_only(_: ProjectionDescriptor<EventualOnly>) {}

    fn assert_direct_state_upsert(game: &BlobGame, expected_event: &str) {
        let occurrence = game.entity.pending_domain_events().last().unwrap();
        assert_eq!(occurrence.descriptor().name, expected_event);
        let state = occurrence.decode_body::<BlobGameState>().unwrap();
        assert_eq!(state, game.state());

        let lowered = BLOB_GAMES
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();
        assert_eq!(lowered.resolved.mutations().len(), 1);
        let [TableMutation::UpsertRow(row)] = lowered.write_plan.mutations.as_slice() else {
            panic!("Blob direct projection must lower to one full-row upsert");
        };
        assert_eq!(row.schema.model_name, "BlobGames");
        assert_eq!(row.schema.table_name, "blob_games");
        assert_eq!(row.values.len(), 8);
    }

    #[test]
    fn initialized_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.initialize("game-1", "alice").unwrap();

        assert_direct_state_upsert(&game, "blob.initialized");
    }

    #[test]
    fn level_started_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.initialize("game-1", "alice").unwrap();
        game.entity.mark_domain_events_committed().unwrap();
        game.start_level("alice", test_map_no_holes()).unwrap();

        assert_direct_state_upsert(&game, "blob.level_started");
    }

    #[test]
    fn started_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.start_with_map("game-1", "alice", test_map_no_holes())
            .unwrap();

        assert_direct_state_upsert(&game, "blob.started");
    }

    #[test]
    fn moved_state_is_one_complete_direct_upsert() {
        let mut game = BlobGame::default();
        game.start_with_map("game-1", "alice", test_map_no_holes())
            .unwrap();
        game.entity.mark_domain_events_committed().unwrap();
        game.move_dir("alice", Direction::Right).unwrap();

        assert_direct_state_upsert(&game, "blob.moved");
        let occurrence = game.entity.pending_domain_events().last().unwrap();
        let lowered = BLOB_GAMES
            .server_executor()
            .unwrap()
            .plan(occurrence)
            .unwrap();
        let [TableMutation::UpsertRow(row)] = lowered.write_plan.mutations.as_slice() else {
            panic!("Blob move must lower to one full-row upsert");
        };
        assert_eq!(row.values.get("score"), Some(&RowValue::I64(1)));
    }

    #[test]
    fn descriptor_is_one_direct_model_across_all_blob_event_arms() {
        let program = BLOB_GAMES.program().unwrap();
        let inventory = BLOB_GAMES.output_inventory().unwrap();

        assert_eq!(program.arms().len(), 4);
        assert!(program.arms().iter().all(|arm| {
            matches!(
                arm.operations(),
                [operation] if operation.kind() == ProjectionMutationKind::Upsert
            )
        }));
        assert_eq!(inventory.models.len(), 1);
        assert_eq!(inventory.models[0].model, "BlobGames");
        assert_eq!(inventory.models[0].storage, "blob_games");
        assert_eq!(BlobGames::schema().table_name, "blob_games");
    }

    #[test]
    fn patch_delete_and_multi_row_descriptors_are_not_direct_candidates() {
        assert_eventual_only(PATCH_BLOB_GAMES);
        assert_eventual_only(DELETE_BLOB_GAMES);
        assert_eventual_only(MULTI_ROW_BLOB_GAMES);

        let patch = PATCH_BLOB_GAMES.program().unwrap();
        let delete = DELETE_BLOB_GAMES.program().unwrap();
        assert_eq!(
            patch.arms()[0].operations()[0].kind(),
            ProjectionMutationKind::Patch
        );
        assert_eq!(
            delete.arms()[0].operations()[0].kind(),
            ProjectionMutationKind::Delete
        );
        let multi_row = MULTI_ROW_BLOB_GAMES.program().unwrap();
        assert_eq!(multi_row.arms()[0].operations().len(), 2);
        assert_eq!(
            MULTI_ROW_BLOB_GAMES
                .output_inventory()
                .unwrap()
                .models
                .len(),
            2
        );
    }
}
