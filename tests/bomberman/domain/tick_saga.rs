use serde::{Deserialize, Serialize};
use sourced_rust::{digest, Entity};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Detonation {
    pub bomb_id: String,
    pub owner: String,
    pub explosion_id: String,
}

#[derive(Default)]
pub struct TickSaga {
    pub entity: Entity,
    pub game_id: String,
    pub bombs_ticked: usize,
    pub detonations: Vec<Detonation>,
    pub blocks_destroyed: Vec<(i32, i32)>,
    pub players_killed: Vec<String>,
    pub chain_detonations: Vec<String>,
    pub explosions_dissipated: Vec<String>,
    pub game_over: bool,
    pub winner: Option<String>,
}

impl TickSaga {
    #[digest("TickStarted")]
    pub fn start(&mut self, saga_id: String, game_id: String, bombs_ticked: usize) {
        self.entity.set_id(&saga_id);
        self.game_id = game_id;
        self.bombs_ticked = bombs_ticked;
    }

    #[digest("DetonationRecorded")]
    pub fn record_detonation(&mut self, detonation: Detonation) {
        self.detonations.push(detonation);
    }

    #[digest("DamageRecorded")]
    pub fn record_damage(
        &mut self,
        blocks_destroyed: Vec<(i32, i32)>,
        players_killed: Vec<String>,
        chain_detonations: Vec<String>,
    ) {
        self.blocks_destroyed.extend(blocks_destroyed);
        self.players_killed.extend(players_killed);
        self.chain_detonations.extend(chain_detonations);
    }

    #[digest("DissipationRecorded")]
    pub fn record_dissipation(&mut self, explosion_id: String) {
        self.explosions_dissipated.push(explosion_id);
    }

    #[digest("TickCompleted")]
    pub fn complete(&mut self, game_over: bool, winner: Option<String>) {
        self.game_over = game_over;
        self.winner = winner;
    }
}

sourced_rust::aggregate!(TickSaga, entity {
    "TickStarted"(saga_id, game_id, bombs_ticked) => start,
    "DetonationRecorded"(detonation) => record_detonation,
    "DamageRecorded"(blocks_destroyed, players_killed, chain_detonations) => record_damage,
    "DissipationRecorded"(explosion_id) => record_dissipation,
    "TickCompleted"(game_over, winner) => complete,
});
