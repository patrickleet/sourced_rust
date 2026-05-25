//! Projects board events into `boards` + `cards`. The board snapshot is the
//! desired state, so `save_changes` collection sync reflects added/moved/removed
//! cards as inserts/patches/deletes. A monotonic `source_version` guard ignores
//! stale snapshots under out-of-order delivery.

use sourced_rust::bus::Event;
use sourced_rust::{InMemoryReadModelStore, ReadModelCommitOutcome, ReadModelUnitOfWorkExt};

use crate::board_service::BoardSnapshot;
use crate::read_models::{board_key, BoardView, CardPayload, CardView};

pub const CONSUMER: &str = "board-detail-projection";
pub const EVENTS: &[&str] = &[
    "board.opened",
    "board.card_added",
    "board.card_moved",
    "board.card_removed",
];

pub fn handle(store: &InMemoryReadModelStore, event: &Event) -> ReadModelCommitOutcome {
    let snapshot: BoardSnapshot = event.decode().expect("board snapshot should decode");
    let version = event_version(event);
    let desired = desired_board_view(&snapshot, version);

    let mut session = store.session();
    let existing = session
        .load::<BoardView>(board_key(&desired.board_id))
        .include("cards")
        .one()
        .expect("board load should succeed");

    match existing {
        Some(current) if current.data.source_version >= version => {}
        Some(_) => {
            session
                .save_changes(desired)
                .expect("board save_changes should stage");
        }
        None => {
            session
                .save(&desired)
                .expect("board root save should stage");
            for card in &desired.cards {
                session.save(card).expect("board card save should stage");
            }
        }
    }

    session.mark_processed(CONSUMER, &event.id);
    session.commit().expect("board projection should commit")
}

fn desired_board_view(snapshot: &BoardSnapshot, version: i64) -> BoardView {
    let cards = snapshot
        .cards
        .iter()
        .map(|card| CardView {
            board_id: snapshot.id.clone(),
            card_id: card.card_id.clone(),
            column: card.column.clone(),
            title: card.title.clone(),
            payload: CardPayload {
                labels: card.labels.clone(),
                assignee: card.assignee.clone(),
            },
            board: None,
        })
        .collect();

    BoardView {
        board_id: snapshot.id.clone(),
        name: snapshot.name.clone(),
        source_version: version,
        cards,
    }
}

/// The aggregate version is the trailing segment of the outbox event id
/// (`<aggregate-id>:<event-type>:<version>`).
fn event_version(event: &Event) -> i64 {
    event
        .id
        .rsplit(':')
        .next()
        .expect("board projection event id should include a version segment")
        .parse()
        .expect("board projection event id should end with a numeric aggregate version")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_version_parses_trailing_outbox_segment() {
        let event =
            Event::with_string_payload("board-1:board.card_added:42", "board.card_added", "{}");

        assert_eq!(event_version(&event), 42);
    }

    #[test]
    #[should_panic(
        expected = "board projection event id should end with a numeric aggregate version"
    )]
    fn event_version_panics_on_malformed_outbox_segment() {
        let event =
            Event::with_string_payload("board-1:board.card_added:bad", "board.card_added", "{}");

        event_version(&event);
    }
}
