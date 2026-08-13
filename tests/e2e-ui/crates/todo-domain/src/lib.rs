//! Todo aggregate — personal task list item owned by one user.
//!
//! Invariants (enforced here, not only in handlers):
//! - every todo has a non-empty id, owner, and title
//! - owner is fixed at create time
//! - complete/reopen/rename only while not archived
//! - archive is terminal for mutations (except re-open is not allowed after archive)

pub mod models;

pub use models::{
    domain_commands, Todo, TodoArchivedDomainEvent, TodoCompletedDomainEvent,
    TodoCreatedDomainEvent, TodoDomainIdentity, TodoError, TodoForceArchivedDomainEvent,
    TodoPurgedDomainEvent, TodoReassignedDomainEvent, TodoRenamedDomainEvent,
    TodoReopenedDomainEvent, TodoState, TodoStatus,
};
