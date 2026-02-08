use sourced_rust::{digest, Entity};

#[derive(Default, Clone)]
pub struct Explosion {
    pub entity: Entity,
    pub bomb_id: String,
    pub owner: String,
    pub center: (i32, i32),
    pub blast_radius: u8,
    pub rings: Vec<Vec<(i32, i32)>>,
    pub current_ring: usize,
    pub active: bool,
}

impl Explosion {
    #[digest("ExplosionStarted")]
    pub fn start(
        &mut self,
        id: String,
        bomb_id: String,
        owner: String,
        center: (i32, i32),
        blast_radius: u8,
        rings: Vec<Vec<(i32, i32)>>,
    ) {
        self.entity.set_id(&id);
        self.bomb_id = bomb_id;
        self.owner = owner;
        self.center = center;
        self.blast_radius = blast_radius;
        self.rings = rings;
        self.current_ring = 0;
        self.active = true;
    }

    #[digest("ExplosionExpanded", when = self.active && !self.is_fully_expanded())]
    pub fn expand(&mut self) {
        self.current_ring += 1;
    }

    #[digest("ExplosionDissipated", when = self.active)]
    pub fn dissipate(&mut self) {
        self.active = false;
    }

    pub fn is_fully_expanded(&self) -> bool {
        self.rings.is_empty() || self.current_ring >= self.rings.len() - 1
    }

    pub fn newly_reached_cells(&self) -> &[(i32, i32)] {
        if self.current_ring < self.rings.len() {
            &self.rings[self.current_ring]
        } else {
            &[]
        }
    }

    pub fn all_active_cells(&self) -> Vec<(i32, i32)> {
        let end = (self.current_ring + 1).min(self.rings.len());
        self.rings[..end].iter().flat_map(|r| r.iter().copied()).collect()
    }
}

sourced_rust::aggregate!(Explosion, entity {
    "ExplosionStarted"(id, bomb_id, owner, center, blast_radius, rings) => start,
    "ExplosionExpanded"() => expand,
    "ExplosionDissipated"() => dissipate,
});
