//! Todo domain models.

mod todo;
mod todo_error;
mod todo_state;
mod todo_status;

pub use todo::{
    domain_commands, Todo, TodoArchivedDomainEvent, TodoCompletedDomainEvent,
    TodoCreatedDomainEvent, TodoDomainIdentity, TodoForceArchivedDomainEvent,
    TodoPurgedDomainEvent, TodoReassignedDomainEvent, TodoRenamedDomainEvent,
    TodoReopenedDomainEvent,
};
pub use todo_error::TodoError;
pub use todo_state::TodoState;
pub use todo_status::TodoStatus;
