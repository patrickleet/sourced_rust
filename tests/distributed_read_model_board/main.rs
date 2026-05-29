//! Distributed read-model example: a kanban board projected into normalized
//! `boards` + `cards` tables, over the async `InMemoryBus`.
//!
//! - the **board service** owns the `Board` aggregate (cards are aggregate
//!   state) and its outbox;
//! - a **projection service** reconciles the relational rows via `sync`
//!   collection sync, so removing a card deletes its `cards` row, and each card
//!   carries a JSONB `payload` column;
//! - a **query service** reads the board with cards (`has_many`) and a card with
//!   its board (`belongs_to`).

mod board_service;
mod projections_service;
mod query_service;
mod read_models;

use std::time::Duration;

use board_service::{AddCard, MoveCard, OpenBoard, RemoveCard};
use projections_service::{load_board, service as build_projection};
use query_service::BoardQueryService;
use read_models::register_schemas;
use serde::Serialize;
use sourced_rust::microsvc::transport::{Bus, BusConsumer, InMemoryBus, RunOptions};
use sourced_rust::microsvc::{Message, MessageKind, Service, Session};
use sourced_rust::{
    AggregateBuilder, AsyncOutboxStore, ClaimOutboxMessages, HashMapOutboxStore, HashMapRepository,
    InMemoryReadModelStore, OutboxClaimRef, Queueable,
};

fn dispatch<D, C>(service: &Service<D>, command: &str, input: C)
where
    D: Send + Sync + 'static,
    C: Serialize,
{
    service
        .dispatch(
            command,
            serde_json::to_value(input).expect("command should encode"),
            Session::new(),
        )
        .unwrap_or_else(|err| panic!("{command} should dispatch: {err:?}"));
}

/// Forward the board's outbox events onto the bus. Board events are fan-out
/// (`domain_event`, no destination), so each is published as an event for the
/// projection's `subscribe` to drain. Payload bytes are forwarded verbatim
/// (bitcode), and the projection decodes them with `BitcodePayloadCodec`.
async fn publish_pending_outbox(outbox: &HashMapOutboxStore, bus: &InMemoryBus) {
    let claimed = outbox
        .claim_async(ClaimOutboxMessages::new(
            "board-outbox-bridge",
            64,
            Duration::from_secs(60),
        ))
        .await
        .expect("outbox claim should succeed");
    for message in claimed {
        bus.publish_message(
            Message::new(
                message.event_type.clone(),
                MessageKind::Event,
                message.payload.clone(),
            )
            .with_id(message.id().to_string()),
        )
        .await
        .expect("board event should publish to the bus");
        let claim = OutboxClaimRef::from_message(&message).expect("claimed message yields a ref");
        outbox
            .complete_async(&claim)
            .await
            .expect("forwarded event should complete");
    }
}

#[tokio::test]
async fn board_service_feeds_a_normalized_card_read_model() {
    let board_store = HashMapRepository::new();
    let board_outbox = board_store.outbox_store();
    let board_service = board_service::model_service(board_store.queued().aggregate());

    let read_store = InMemoryReadModelStore::new();
    register_schemas(&read_store).expect("relational schemas should register");
    let projection = build_projection(read_store.clone());
    let query_service = BoardQueryService::new(read_store.clone());

    dispatch(
        &board_service,
        "board.open",
        OpenBoard {
            id: "board-1".to_string(),
            name: "Roadmap".to_string(),
        },
    );
    dispatch(
        &board_service,
        "board.add_card",
        AddCard {
            id: "board-1".to_string(),
            card_id: "card-spec".to_string(),
            column: "todo".to_string(),
            title: "Write spec".to_string(),
            labels: vec!["design".to_string()],
            assignee: Some("ada".to_string()),
        },
    );
    dispatch(
        &board_service,
        "board.add_card",
        AddCard {
            id: "board-1".to_string(),
            card_id: "card-impl".to_string(),
            column: "todo".to_string(),
            title: "Implement".to_string(),
            labels: vec!["code".to_string()],
            assignee: None,
        },
    );
    dispatch(
        &board_service,
        "board.move_card",
        MoveCard {
            id: "board-1".to_string(),
            card_id: "card-spec".to_string(),
            column: "doing".to_string(),
        },
    );
    dispatch(
        &board_service,
        "board.remove_card",
        RemoveCard {
            id: "board-1".to_string(),
            card_id: "card-impl".to_string(),
        },
    );

    // Forward the board's outbox events onto the bus, then drain them into the
    // projection in one pass. The projection's monotonic `source_version` guard
    // makes the per-event-type drain order-independent (the highest version
    // wins), so the final board reflects every processed event.
    let bus = InMemoryBus::new();
    publish_pending_outbox(&board_outbox, &bus).await;
    bus.subscribe(projection, RunOptions::idempotent())
        .await
        .expect("projection should drain the board events");

    let board = load_board(&read_store, "board-1").expect("board should be projected");

    assert_eq!(board.name, "Roadmap");
    assert_eq!(board.cards.len(), 1, "removed card should be deleted");
    let card = &board.cards[0];
    assert_eq!(card.card_id, "card-spec");
    assert_eq!(card.column, "doing");
    // JSONB payload round-trips structured data.
    assert_eq!(card.payload.labels, vec!["design".to_string()]);
    assert_eq!(card.payload.assignee.as_deref(), Some("ada"));

    // belongs_to include resolves the card's board.
    let card_with_board = query_service
        .card_with_board("board-1", "card-spec")
        .expect("query should succeed")
        .expect("card should exist");
    let parent = card_with_board
        .board
        .expect("belongs_to board should hydrate");
    assert_eq!(parent.board_id, "board-1");
    assert_eq!(parent.name, "Roadmap");

    // The removed card's row is gone.
    assert!(query_service
        .board_with_cards("board-1")
        .expect("query should succeed")
        .expect("board should exist")
        .cards
        .iter()
        .all(|card| card.card_id != "card-impl"));
    assert!(query_service
        .card_with_board("board-1", "card-impl")
        .expect("query should succeed")
        .is_none());

    let write_side = board_service
        .repo()
        .peek("board-1")
        .expect("write-side load should succeed")
        .expect("write-side board should exist");
    assert_eq!(write_side.cards.len(), 1);
}
