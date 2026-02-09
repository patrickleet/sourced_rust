//! Basic domain service dispatch tests.

use sourced_rust::{
    CommitAggregate, DomainService, GetAggregate, HandlerError, HashMapRepository, Message,
};

use crate::support::{Counter, CreateCounter, DecrementCounter, IncrementCounter};

// ============================================================================
// Test 1: Register and dispatch a single command
// ============================================================================

#[test]
fn dispatch_single_command() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo).command("counter.create", |repo, msg| {
        let data: CreateCounter = msg.decode()?;
        let mut counter = Counter::default();
        counter.create(data.id);
        repo.commit_aggregate(&mut counter)?;
        Ok(())
    });

    let msg = Message::encode("cmd-1", "counter.create", &CreateCounter { id: "c1".into() })
        .unwrap();
    service.handle(&msg).unwrap();

    // Verify aggregate was created
    let counter: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 0);
}

// ============================================================================
// Test 2: Multiple commands, full lifecycle
// ============================================================================

#[test]
fn dispatch_multiple_commands() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo)
        .command("counter.create", |repo, msg| {
            let data: CreateCounter = msg.decode()?;
            let mut counter = Counter::default();
            counter.create(data.id);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        })
        .command("counter.increment", |repo, msg| {
            let data: IncrementCounter = msg.decode()?;
            let mut counter: Counter = repo
                .get_aggregate(&data.id)?
                .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
            counter.increment(data.amount);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        })
        .command("counter.decrement", |repo, msg| {
            let data: DecrementCounter = msg.decode()?;
            let mut counter: Counter = repo
                .get_aggregate(&data.id)?
                .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
            counter.decrement(data.amount);
            repo.commit_aggregate(&mut counter)?;
            Ok(())
        });

    // Create
    let msg = Message::encode("cmd-1", "counter.create", &CreateCounter { id: "c1".into() })
        .unwrap();
    service.handle(&msg).unwrap();

    // Increment twice
    let msg = Message::encode(
        "cmd-2",
        "counter.increment",
        &IncrementCounter {
            id: "c1".into(),
            amount: 5,
        },
    )
    .unwrap();
    service.handle(&msg).unwrap();

    let msg = Message::encode(
        "cmd-3",
        "counter.increment",
        &IncrementCounter {
            id: "c1".into(),
            amount: 3,
        },
    )
    .unwrap();
    service.handle(&msg).unwrap();

    // Decrement
    let msg = Message::encode(
        "cmd-4",
        "counter.decrement",
        &DecrementCounter {
            id: "c1".into(),
            amount: 2,
        },
    )
    .unwrap();
    service.handle(&msg).unwrap();

    // Verify final state: 0 + 5 + 3 - 2 = 6
    let counter: Counter = service.repo().get_aggregate("c1").unwrap().unwrap();
    assert_eq!(counter.value, 6);
}

// ============================================================================
// Test 3: Unknown command returns error
// ============================================================================

#[test]
fn unknown_command_returns_error() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo).command("counter.create", |_repo, _msg| Ok(()));

    let msg = Message::with_string_payload("cmd-1", "counter.unknown", "{}");
    let result = service.handle(&msg);

    assert!(result.is_err());
    match result.unwrap_err() {
        HandlerError::UnknownCommand(name) => assert_eq!(name, "counter.unknown"),
        other => panic!("Expected UnknownCommand, got: {:?}", other),
    }
}

// ============================================================================
// Test 4: Handler errors propagate correctly
// ============================================================================

#[test]
fn handler_errors_propagate() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo).command("counter.increment", |repo, msg| {
        let data: IncrementCounter = msg.decode()?;
        let _counter: Counter = repo
            .get_aggregate(&data.id)?
            .ok_or_else(|| HandlerError::NotFound(data.id.clone()))?;
        Ok(())
    });

    // Try to increment a non-existent counter
    let msg = Message::encode(
        "cmd-1",
        "counter.increment",
        &IncrementCounter {
            id: "nonexistent".into(),
            amount: 1,
        },
    )
    .unwrap();
    let result = service.handle(&msg);

    assert!(result.is_err());
    match result.unwrap_err() {
        HandlerError::NotFound(id) => assert_eq!(id, "nonexistent"),
        other => panic!("Expected NotFound, got: {:?}", other),
    }
}

// ============================================================================
// Test 5: Decode error from bad payload
// ============================================================================

#[test]
fn decode_error_from_bad_payload() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo).command("counter.create", |_repo, msg| {
        let _data: CreateCounter = msg.decode()?;
        Ok(())
    });

    // Send a message with invalid payload (not valid bitcode)
    let msg = Message::with_string_payload("cmd-1", "counter.create", "not-bitcode");
    let result = service.handle(&msg);

    assert!(result.is_err());
    match result.unwrap_err() {
        HandlerError::DecodeFailed(_) => {}
        other => panic!("Expected DecodeFailed, got: {:?}", other),
    }
}

// ============================================================================
// Test 6: commands() lists registered handlers
// ============================================================================

#[test]
fn commands_lists_registered_handlers() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo)
        .command("counter.create", |_, _| Ok(()))
        .command("counter.increment", |_, _| Ok(()))
        .command("counter.decrement", |_, _| Ok(()));

    let mut cmds = service.commands();
    cmds.sort();
    assert_eq!(cmds, vec!["counter.create", "counter.decrement", "counter.increment"]);
}

// ============================================================================
// Test 7: Rejected error from handler
// ============================================================================

#[test]
fn rejected_error_from_handler() {
    let repo = HashMapRepository::new();
    let service = DomainService::new(repo).command("counter.create", |_repo, _msg| {
        Err(HandlerError::Rejected("duplicate counter".into()))
    });

    let msg = Message::encode("cmd-1", "counter.create", &CreateCounter { id: "c1".into() })
        .unwrap();
    let result = service.handle(&msg);

    assert!(result.is_err());
    match result.unwrap_err() {
        HandlerError::Rejected(reason) => assert_eq!(reason, "duplicate counter"),
        other => panic!("Expected Rejected, got: {:?}", other),
    }
}
