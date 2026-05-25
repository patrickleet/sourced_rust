use sourced_rust::{impl_aggregate, Entity, EventRecord};

use super::checkout::{
    AddSeat, SeatAdded, SeatReserved, SEAT_ADDED, SEAT_AVAILABLE_STATUS, SEAT_RESERVED,
    SEAT_RESERVED_STATUS,
};

#[derive(Default)]
pub struct Seat {
    pub entity: Entity,
    pub category: String,
    pub status: String,
    pub checkout_id: String,
}

impl Seat {
    pub fn add(&mut self, command: AddSeat) -> Result<SeatAdded, String> {
        let event = SeatAdded {
            seat_id: command.seat_id,
            category: command.category,
        };
        self.entity.set_id(&event.seat_id);
        self.entity
            .digest(SEAT_ADDED, &event)
            .map_err(|err| err.to_string())?;
        self.apply_added(&event);
        Ok(event)
    }

    pub fn reserve(&mut self, checkout_id: String) -> Result<SeatReserved, String> {
        if self.status != SEAT_AVAILABLE_STATUS {
            return Err(format!("seat {} is not available", self.entity.id()));
        }

        let event = SeatReserved {
            checkout_id,
            seat_id: self.entity.id().to_string(),
            seat_category: self.category.clone(),
        };
        self.entity
            .digest(SEAT_RESERVED, &event)
            .map_err(|err| err.to_string())?;
        self.apply_reserved(&event);
        Ok(event)
    }

    fn replay(&mut self, event: &EventRecord) -> Result<(), String> {
        match event.event_name.as_str() {
            SEAT_ADDED => {
                let event = event.decode::<SeatAdded>().map_err(|err| err.to_string())?;
                self.apply_added(&event);
            }
            SEAT_RESERVED => {
                let event = event
                    .decode::<SeatReserved>()
                    .map_err(|err| err.to_string())?;
                self.apply_reserved(&event);
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_added(&mut self, event: &SeatAdded) {
        self.category = event.category.clone();
        self.status = SEAT_AVAILABLE_STATUS.to_string();
        self.checkout_id.clear();
    }

    fn apply_reserved(&mut self, event: &SeatReserved) {
        self.status = SEAT_RESERVED_STATUS.to_string();
        self.checkout_id = event.checkout_id.clone();
    }
}

impl_aggregate!(Seat, entity, replay, aggregate_type = "conformance.seat");
