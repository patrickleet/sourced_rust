//! Projects board events into `boards` + `cards`. The board snapshot is the
//! updated view, so workspace sync reflects added/moved/removed cards as
//! inserts/patches/deletes. A monotonic `source_version` guard ignores stale
//! snapshots under out-of-order delivery.

use serde_json::{json, Value};
use sourced_rust::bus::Event;
use sourced_rust::microsvc::{Context, HandlerError};
use sourced_rust::ReadModelWorkspaceExt;

use crate::board_service::BoardSnapshot;
use crate::projections_service::{read_model_error, ProjectionDependencies};
use crate::read_models::{board_key, BoardView, CardPayload, CardView};

pub const CONSUMER: &str = "board-detail-projection";
pub const EVENTS: &[&str] = &[
    "board.opened",
    "board.card_added",
    "board.card_moved",
    "board.card_removed",
];
pub const SPEC: sourced_rust::microsvc::HandlerSpec =
    sourced_rust::microsvc::HandlerSpec::events(EVENTS).envelope();

pub fn guard(ctx: &Context<ProjectionDependencies>) -> bool {
    ctx.has_fields(&["id", "name", "payload"])
}

pub fn handle(ctx: &Context<ProjectionDependencies>) -> Result<Value, HandlerError> {
    let event = super::event(ctx)?;
    let snapshot: BoardSnapshot = event
        .decode()
        .map_err(|err| HandlerError::DecodeFailed(format!("board snapshot: {err}")))?;
    let version = event_version(&event);
    let updated_view = updated_board_view(&snapshot, version);

    let mut workspace = ctx.read_model_store().workspace();
    let existing = workspace
        .load::<BoardView>(board_key(&updated_view.board_id))
        .include("cards")
        .one()
        .map_err(read_model_error)?;

    match existing {
        Some(current) if current.data.source_version >= version => {}
        Some(_) => {
            workspace.sync(updated_view).map_err(read_model_error)?;
        }
        None => {
            workspace.upsert(&updated_view).map_err(read_model_error)?;
            for card in &updated_view.cards {
                workspace.upsert(card).map_err(read_model_error)?;
            }
        }
    }

    workspace.mark_processed(CONSUMER, &event.id);
    workspace.commit().map_err(read_model_error)?;

    Ok(json!({ "event_id": event.id }))
}

fn updated_board_view(snapshot: &BoardSnapshot, version: i64) -> BoardView {
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
