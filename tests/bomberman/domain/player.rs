use sourced_rust::{digest, Entity};

use super::types::PowerUp;

#[derive(Default, Clone)]
pub struct Player {
    pub entity: Entity,
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub alive: bool,
    pub max_bombs: u8,
    pub active_bombs: u8,
    pub blast_radius: u8,
}

impl Player {
    #[digest("PlayerJoined")]
    pub fn join(&mut self, id: String, name: String, x: i32, y: i32) {
        self.entity.set_id(&id);
        self.name = name;
        self.x = x;
        self.y = y;
        self.alive = true;
        self.max_bombs = 1;
        self.active_bombs = 0;
        self.blast_radius = 2;
    }

    #[digest("PlayerMoved", when = self.alive)]
    pub fn move_to(&mut self, x: i32, y: i32) {
        self.x = x;
        self.y = y;
    }

    #[digest("PlayerKilled", when = self.alive)]
    pub fn kill(&mut self) {
        self.alive = false;
    }

    #[digest("BombPlaced", when = self.alive && self.active_bombs < self.max_bombs)]
    pub fn place_bomb(&mut self) {
        self.active_bombs += 1;
    }

    #[digest("BombReturned")]
    pub fn return_bomb(&mut self) {
        if self.active_bombs > 0 {
            self.active_bombs -= 1;
        }
    }

    #[digest("PowerUpApplied")]
    pub fn apply_power_up(&mut self, power_up: PowerUp) {
        match power_up {
            PowerUp::BombUp => self.max_bombs += 1,
            PowerUp::FireUp => self.blast_radius += 1,
        }
    }

    pub fn can_place_bomb(&self) -> bool {
        self.alive && self.active_bombs < self.max_bombs
    }
}

sourced_rust::aggregate!(Player, entity {
    "PlayerJoined"(id, name, x, y) => join,
    "PlayerMoved"(x, y) => move_to,
    "PlayerKilled"() => kill,
    "BombPlaced"() => place_bomb,
    "BombReturned"() => return_bomb,
    "PowerUpApplied"(power_up) => apply_power_up,
});
