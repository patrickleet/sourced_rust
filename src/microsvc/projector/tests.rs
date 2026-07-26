use std::error::Error as _;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::bus::{Bus, FailurePolicy, InMemoryBus, RunOptions};
use crate::bus::{Message, MessageKind};
use crate::graphql::SurfaceProjector;
use crate::microsvc::{CausalProjectorContext, HandlerError, ProjectionRepairHandle, Routes};
use crate::projection_protocol::{
    CompiledProjectionTopology, ProjectionCheckpointProbe, ProjectionGeneration,
    ProjectionInputCursor, ProjectionProtocolStore, ProjectionQuerySnapshotRequest,
};
use crate::table::{RowKey, RowValue};
use crate::{InMemoryRepository, RelationalReadModel};

use crate::microsvc::Service;

const FACT_NAME: &str = "task15.todo_changed";

#[derive(Clone, Debug, serde::Deserialize)]
struct TodoChanged {
    id: String,
    title: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel)]
#[readmodel(table = "task15_primary_views", primary_key = ["id"])]
struct PrimaryView {
    id: String,
    title: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel)]
#[readmodel(table = "task15_secondary_views", primary_key = ["id"])]
struct SecondaryView {
    id: String,
    title: String,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, crate::ReadModel)]
#[readmodel(table = "task15_malformed_views", primary_key = ["id"])]
struct MalformedView {
    id: String,
}

fn primary_projector() -> SurfaceProjector {
    SurfaceProjector::new("task15_a_primary")
        .facts([FACT_NAME])
        .models(["PrimaryView"])
        .change_epoch("task15-primary-v1")
}

fn secondary_projector() -> SurfaceProjector {
    SurfaceProjector::new("task15_b_secondary")
        .facts([FACT_NAME])
        .models(["SecondaryView"])
        .change_epoch("task15-secondary-v1")
}

fn fact_message(id: &str, title: &str) -> Message {
    Message::new(
        FACT_NAME,
        MessageKind::Event,
        serde_json::to_vec(&serde_json::json!({ "id": id, "title": title })).unwrap(),
    )
    .with_id(format!("fact-{id}"))
    .with_metadata(crate::trace_context::CAUSATION_ID, format!("command-{id}"))
}

fn row_key(id: &str) -> RowKey {
    RowKey::new([("id", RowValue::String(id.to_string()))])
}

#[tokio::test]
async fn public_builder_fans_one_fact_out_and_replays_partial_success_exactly() {
    let repository = InMemoryRepository::new();
    let bus = InMemoryBus::new();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let fail_secondary_once = Arc::new(AtomicBool::new(true));

    let primary = primary_projector();
    let secondary = secondary_projector();
    let primary_calls = Arc::clone(&calls);
    let secondary_calls = Arc::clone(&calls);
    let secondary_failure = Arc::clone(&fail_secondary_once);
    let routes = Routes::new()
        .with_read_model_store(repository.clone())
        .causal_projector::<TodoChanged>(primary.clone())
        .model::<PrimaryView>()
        .handle(move |context: CausalProjectorContext, fact: TodoChanged| {
            let calls = Arc::clone(&primary_calls);
            async move {
                {
                    let mut calls = calls.lock().unwrap();
                    if calls.contains(&"primary") {
                        return Err(HandlerError::Rejected(
                            "an applied projector must not be reinvoked on sibling retry".into(),
                        ));
                    }
                    calls.push("primary");
                }
                context
                    .project(&PrimaryView {
                        id: fact.id,
                        title: fact.title,
                    })
                    .await?;
                Ok(())
            }
        })
        .causal_projector::<TodoChanged>(secondary)
        .model::<SecondaryView>()
        .handle(move |context: CausalProjectorContext, fact: TodoChanged| {
            let calls = Arc::clone(&secondary_calls);
            let fail = Arc::clone(&secondary_failure);
            async move {
                calls.lock().unwrap().push("secondary");
                if fail.swap(false, Ordering::SeqCst) {
                    return Err(HandlerError::Other(
                        "injected transient projector failure".into(),
                    ));
                }
                context
                    .project(&SecondaryView {
                        id: fact.id,
                        title: fact.title,
                    })
                    .await?;
                Ok(())
            }
        });
    let service = Service::new().routes(routes).with_bus(bus.clone());
    bus.publish_message(fact_message("todo-1", "causal cache"))
        .await
        .unwrap();

    service.run(RunOptions::idempotent()).await.unwrap();
    assert_eq!(
        *calls.lock().unwrap(),
        vec!["primary", "secondary", "secondary"],
        "an applied sibling is skipped while only the failed projector retries"
    );

    let compiled = CompiledProjectionTopology::compile(
        &primary.name,
        &primary.facts,
        &primary.models,
        &primary.partition,
        [PrimaryView::schema()],
    )
    .unwrap();
    let codec = compiled.codec();
    let partition = codec.encode_partition(None).unwrap();
    let ordered = bus.ordered_topic_evidence(FACT_NAME, 0);
    let cursor = ProjectionInputCursor::new(
        compiled.topology().clone(),
        partition.clone(),
        ordered.source().clone(),
        ordered.epoch().clone(),
        ordered.position(),
    )
    .unwrap();
    let snapshot = repository
        .projection_query_snapshot(
            &ProjectionQuerySnapshotRequest::new(
                &codec,
                None,
                "PrimaryView",
                row_key("todo-1"),
                vec![ProjectionCheckpointProbe::new(
                    compiled.topology().clone(),
                    partition,
                    ordered.source().clone(),
                    ordered.epoch().clone(),
                    ProjectionGeneration::initial(),
                )],
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let row = snapshot.row.expect("projected physical row");
    assert_eq!(row.get_serde::<String>("title").unwrap(), "causal cache");
    let record = snapshot.record.expect("projection record revision");
    assert_eq!(record.revision.incarnation(), 1);
    assert_eq!(record.revision.revision(), 1);
    let checkpoint = snapshot.checkpoints[0]
        .checkpoint
        .as_ref()
        .expect("source checkpoint");
    assert_eq!(checkpoint.input(), &cursor);
    assert!(checkpoint.is_gap_free());
}

#[tokio::test]
async fn service_fans_same_fact_out_across_projector_only_route_bundles() {
    static INFERRED_PRIMARY_CALLS: AtomicUsize = AtomicUsize::new(0);

    let repository = InMemoryRepository::new();
    let bus = InMemoryBus::new();
    let secondary_calls = Arc::new(AtomicUsize::new(0));
    INFERRED_PRIMARY_CALLS.store(0, Ordering::SeqCst);

    let primary_routes = Routes::new()
        .with_read_model_store(repository.clone())
        .causal_projector::<TodoChanged>(primary_projector())
        .model::<PrimaryView>()
        .handle(|context, fact: TodoChanged| async move {
            INFERRED_PRIMARY_CALLS.fetch_add(1, Ordering::SeqCst);
            context
                .project(&PrimaryView {
                    id: fact.id,
                    title: fact.title,
                })
                .await?;
            Ok(())
        });
    let secondary_seen = Arc::clone(&secondary_calls);
    let secondary_routes = Routes::new()
        .with_read_model_store(repository)
        .causal_projector::<TodoChanged>(secondary_projector())
        .model::<SecondaryView>()
        .handle(move |context: CausalProjectorContext, fact: TodoChanged| {
            let seen = Arc::clone(&secondary_seen);
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                context
                    .project(&SecondaryView {
                        id: fact.id,
                        title: fact.title,
                    })
                    .await?;
                Ok(())
            }
        });
    let service = Service::new()
        .routes(primary_routes)
        .routes(secondary_routes)
        .with_bus(bus.clone());
    assert_eq!(
        service.subscription_plan().events,
        vec![FACT_NAME.to_string()]
    );
    bus.publish_message(fact_message("todo-2", "cross bundle"))
        .await
        .unwrap();
    service.run(RunOptions::idempotent()).await.unwrap();

    assert_eq!(INFERRED_PRIMARY_CALLS.load(Ordering::SeqCst), 1);
    assert_eq!(secondary_calls.load(Ordering::SeqCst), 1);
}

fn malformed_service(repository: InMemoryRepository, invoked: Arc<AtomicUsize>) -> Service {
    let projector = SurfaceProjector::new("task15_malformed")
        .facts(["task15.malformed"])
        .models(["MalformedView"])
        .change_epoch("task15-malformed-v1")
        .partition_by(["tenant"]);
    Service::new().routes(
        Routes::new()
            .with_read_model_store(repository)
            .causal_projector::<TodoChanged>(projector)
            .model::<MalformedView>()
            .handle(
                move |_context: CausalProjectorContext, _fact: TodoChanged| {
                    let invoked = Arc::clone(&invoked);
                    async move {
                        invoked.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    }
                },
            ),
    )
}

fn repair_handle(error: &crate::bus::TransportError) -> ProjectionRepairHandle {
    error
        .source()
        .and_then(|source| source.downcast_ref::<HandlerError>())
        .and_then(HandlerError::projection_repair_handle)
        .cloned()
        .expect("terminal transport error carries an operator repair handle")
}

#[tokio::test]
async fn malformed_ingress_emits_safe_handle_and_repair_restart_stays_terminal() {
    let repository = InMemoryRepository::new();
    let bus = InMemoryBus::new();
    let invoked = Arc::new(AtomicUsize::new(0));
    let malformed = Message::new(
        "task15.malformed",
        MessageKind::Event,
        b"{tenant-secret".to_vec(),
    )
    .with_id("malformed-1")
    .with_metadata(crate::trace_context::CAUSATION_ID, "command-malformed");
    bus.publish_message(malformed).await.unwrap();

    let first = malformed_service(repository.clone(), Arc::clone(&invoked))
        .with_bus(bus.clone())
        .run(RunOptions::idempotent())
        .await
        .expect_err("malformed ingress must retain its exact position and stop");
    let first_handle = repair_handle(&first);
    let token = first_handle.to_string();
    assert!(!token.contains("tenant-secret"));
    assert_eq!(
        token.parse::<ProjectionRepairHandle>().unwrap(),
        first_handle
    );
    assert_eq!(
        serde_json::from_str::<ProjectionRepairHandle>(
            &serde_json::to_string(&first_handle).unwrap()
        )
        .unwrap(),
        first_handle
    );

    let repaired = malformed_service(repository, Arc::clone(&invoked));
    assert_eq!(
        repaired
            .repair_projection(&first_handle)
            .await
            .unwrap()
            .get(),
        2
    );
    let second = repaired
        .with_bus(bus)
        .run(RunOptions::idempotent())
        .await
        .expect_err("unchanged malformed bytes must stop the repaired generation again");
    let second_handle = repair_handle(&second);
    assert_ne!(second_handle, first_handle);
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
}

#[test]
fn repair_handle_parser_rejects_noncanonical_or_non_hash_tokens() {
    for token in [
        "",
        "distributed-repair-v2:abcd",
        "distributed-repair-v1:abcd",
        "distributed-repair-v1:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    ] {
        assert!(token.parse::<ProjectionRepairHandle>().is_err(), "{token}");
    }
}

#[tokio::test]
async fn unrecorded_permanent_projector_failure_retains_and_stops_under_drop_policies() {
    for policy in [FailurePolicy::DeadLetter, FailurePolicy::LogAndAck] {
        let repository = InMemoryRepository::new();
        let bus = InMemoryBus::new();
        let invoked = Arc::new(AtomicUsize::new(0));
        let handler_invoked = Arc::clone(&invoked);
        let service = Service::new()
            .routes(
                Routes::new()
                    .with_read_model_store(repository)
                    .causal_projector::<TodoChanged>(primary_projector())
                    .model::<PrimaryView>()
                    .handle(move |_context, _fact| {
                        let invoked = Arc::clone(&handler_invoked);
                        async move {
                            invoked.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        }
                    }),
            )
            .with_bus(bus.clone());
        bus.publish_message(Message::new(
            FACT_NAME,
            MessageKind::Event,
            br#"{"id":"secret-tenant","title":"must not cross"}"#.to_vec(),
        ))
        .await
        .unwrap();

        let error = service
            .run(RunOptions::idempotent().with_failure_policy(policy))
            .await
            .expect_err("unrecorded permanent projector failure must stop the runner");
        assert!(error.is_permanent());
        assert!(error.should_retain_and_stop());
        let halted = error
            .source()
            .and_then(|source| source.downcast_ref::<HandlerError>());
        assert!(
            matches!(halted, Some(HandlerError::ProjectionDeliveryHalted { .. })),
            "projector-only dispatch must erase the unrecorded internal failure"
        );
        assert!(
            matches!(
                halted
                    .and_then(|halted| halted.source())
                    .and_then(|source| source.downcast_ref::<HandlerError>()),
                Some(HandlerError::UnqualifiedProjectionDelivery(_))
            ),
            "operator diagnostics retain the original failure only as an error source"
        );
        assert!(!error.to_string().contains("secret-tenant"));
        assert_eq!(invoked.load(Ordering::SeqCst), 0);
    }
}
