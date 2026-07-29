//! Todo domain models.

mod todo;
mod todo_error;
mod todo_state;
mod todo_status;
mod todos;

pub use todo::{
    Todo, TodoArchivedDomainEvent, TodoCompletedDomainEvent, TodoCreatedDomainEvent,
    TodoDomainIdentity, TodoForceArchivedDomainEvent, TodoPurgedDomainEvent,
    TodoReassignedDomainEvent, TodoRenamedDomainEvent, TodoReopenedDomainEvent,
};
pub use todo_error::TodoError;
pub use todo_state::TodoState;
pub use todo_status::TodoStatus;
pub use todos::Todos;
