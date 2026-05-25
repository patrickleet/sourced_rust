use sourced_rust::{impl_aggregate, Entity, EventRecord};

use super::checkout::{
    CheckoutStarted, SeatReservationCompleted, SeatReserved, StartCheckout,
    CHECKOUT_SEAT_RESERVATION_COMPLETED, CHECKOUT_SEAT_RESERVED_STATUS, CHECKOUT_STARTED,
    CHECKOUT_STARTED_STATUS,
};

#[derive(Default)]
pub struct CheckoutSaga {
    pub entity: Entity,
    pub status: String,
    pub seat_id: String,
    pub seat_category: String,
    pub reserved_seat_id: String,
}

impl CheckoutSaga {
    pub fn start(&mut self, command: StartCheckout) -> Result<CheckoutStarted, String> {
        let event = CheckoutStarted {
            checkout_id: command.checkout_id,
            seat_id: command.seat_id,
            seat_category: command.seat_category,
        };
        self.entity.set_id(&event.checkout_id);
        self.entity
            .digest(CHECKOUT_STARTED, &event)
            .map_err(|err| err.to_string())?;
        self.apply_started(&event);
        Ok(event)
    }

    pub fn record_seat_reserved(
        &mut self,
        event: SeatReserved,
    ) -> Result<SeatReservationCompleted, String> {
        let completed = SeatReservationCompleted {
            checkout_id: event.checkout_id,
            seat_id: event.seat_id,
            seat_category: event.seat_category,
        };
        self.entity
            .digest(CHECKOUT_SEAT_RESERVATION_COMPLETED, &completed)
            .map_err(|err| err.to_string())?;
        self.apply_seat_reservation_completed(&completed);
        Ok(completed)
    }

    fn replay(&mut self, event: &EventRecord) -> Result<(), String> {
        match event.event_name.as_str() {
            CHECKOUT_STARTED => {
                let event = event
                    .decode::<CheckoutStarted>()
                    .map_err(|err| err.to_string())?;
                self.apply_started(&event);
            }
            CHECKOUT_SEAT_RESERVATION_COMPLETED => {
                let event = event
                    .decode::<SeatReservationCompleted>()
                    .map_err(|err| err.to_string())?;
                self.apply_seat_reservation_completed(&event);
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_started(&mut self, event: &CheckoutStarted) {
        self.status = CHECKOUT_STARTED_STATUS.to_string();
        self.seat_id = event.seat_id.clone();
        self.seat_category = event.seat_category.clone();
    }

    fn apply_seat_reservation_completed(&mut self, event: &SeatReservationCompleted) {
        self.status = CHECKOUT_SEAT_RESERVED_STATUS.to_string();
        self.reserved_seat_id = event.seat_id.clone();
    }
}

impl_aggregate!(
    CheckoutSaga,
    entity,
    replay,
    aggregate_type = "conformance.checkout_saga"
);
