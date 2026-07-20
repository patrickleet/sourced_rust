use distributed::{sourced, Entity};
use serde::{Deserialize, Serialize};

use super::{TodoError, TodoStatus};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Todo {
    #[serde(skip, default)]
    pub entity: Entity,
    pub todo_id: String,
    pub owner_id: String,
    pub title: String,
    pub status: TodoStatus,
}

impl Todo {
    pub fn is_created(&self) -> bool {
        !self.todo_id.is_empty()
    }

    pub fn ensure_owner(&self, owner_id: &str) -> Result<(), TodoError> {
        if !self.is_created() {
            return Err(TodoError::NotCreated);
        }
        if self.owner_id != owner_id {
            return Err(TodoError::NotOwner);
        }
        Ok(())
    }

    fn require_mutable(&self) -> Result<(), TodoError> {
        if !self.is_created() {
            return Err(TodoError::NotCreated);
        }
        if matches!(self.status, TodoStatus::Archived) {
            return Err(TodoError::Archived);
        }
        Ok(())
    }
}

#[sourced(entity, events = "TodoEvent", aggregate_type = "todo")]
impl Todo {
    /// Create a new open todo. `owner_id` is the authenticated user (never trusted from peers).
    pub fn create(
        &mut self,
        todo_id: impl Into<String>,
        owner_id: impl Into<String>,
        title: impl Into<String>,
    ) -> Result<(), TodoError> {
        if self.is_created() {
            return Err(TodoError::AlreadyExists);
        }
        let todo_id = todo_id.into();
        let owner_id = owner_id.into();
        let title = title.into();
        if todo_id.trim().is_empty() {
            return Err(TodoError::EmptyId);
        }
        if owner_id.trim().is_empty() {
            return Err(TodoError::EmptyOwner);
        }
        let title = title.trim();
        if title.is_empty() {
            return Err(TodoError::EmptyTitle);
        }
        self.record_created(todo_id, owner_id, title.to_string())?;
        Ok(())
    }

    #[event("todo.created")]
    fn record_created(&mut self, todo_id: String, owner_id: String, title: String) {
        self.entity.set_id(&todo_id);
        self.todo_id = todo_id;
        self.owner_id = owner_id;
        self.title = title;
        self.status = TodoStatus::Open;
    }

    pub fn rename(&mut self, owner_id: &str, title: impl Into<String>) -> Result<(), TodoError> {
        self.ensure_owner(owner_id)?;
        self.require_mutable()?;
        let title = title.into();
        let title = title.trim();
        if title.is_empty() {
            return Err(TodoError::EmptyTitle);
        }
        if title == self.title {
            return Ok(()); // no-op: same title
        }
        self.record_renamed(title.to_string())?;
        Ok(())
    }

    #[event("todo.renamed")]
    fn record_renamed(&mut self, title: String) {
        self.title = title;
    }

    pub fn complete(&mut self, owner_id: &str) -> Result<(), TodoError> {
        self.ensure_owner(owner_id)?;
        self.require_mutable()?;
        if matches!(self.status, TodoStatus::Completed) {
            return Err(TodoError::AlreadyCompleted);
        }
        self.record_completed()?;
        Ok(())
    }

    #[event("todo.completed")]
    fn record_completed(&mut self) {
        self.status = TodoStatus::Completed;
    }

    pub fn reopen(&mut self, owner_id: &str) -> Result<(), TodoError> {
        self.ensure_owner(owner_id)?;
        self.require_mutable()?;
        if !matches!(self.status, TodoStatus::Completed) {
            return Err(TodoError::NotCompleted);
        }
        self.record_reopened()?;
        Ok(())
    }

    #[event("todo.reopened")]
    fn record_reopened(&mut self) {
        self.status = TodoStatus::Open;
    }

    pub fn archive(&mut self, owner_id: &str) -> Result<(), TodoError> {
        self.ensure_owner(owner_id)?;
        if !self.is_created() {
            return Err(TodoError::NotCreated);
        }
        if matches!(self.status, TodoStatus::Archived) {
            return Ok(()); // idempotent archive
        }
        self.record_archived()?;
        Ok(())
    }

    #[event("todo.archived")]
    fn record_archived(&mut self) {
        self.status = TodoStatus::Archived;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_todo() -> Todo {
        let mut t = Todo::default();
        t.create("t1", "alice", "Buy milk").unwrap();
        t
    }

    #[test]
    fn create_sets_owner_and_open_status() {
        let t = open_todo();
        assert!(t.is_created());
        assert_eq!(t.owner_id, "alice");
        assert_eq!(t.title, "Buy milk");
        assert_eq!(t.status, TodoStatus::Open);
        assert_eq!(t.entity.id(), "t1");
        assert_eq!(t.entity.version(), 1);
    }

    #[test]
    fn rejects_empty_title_and_double_create() {
        let mut t = Todo::default();
        assert_eq!(t.create("t1", "alice", "  ").unwrap_err(), TodoError::EmptyTitle);
        t.create("t1", "alice", "ok").unwrap();
        assert_eq!(
            t.create("t1", "alice", "again").unwrap_err(),
            TodoError::AlreadyExists
        );
    }

    #[test]
    fn only_owner_can_mutate() {
        let mut t = open_todo();
        assert_eq!(t.complete("bob").unwrap_err(), TodoError::NotOwner);
        t.complete("alice").unwrap();
        assert_eq!(t.status, TodoStatus::Completed);
    }

    #[test]
    fn complete_reopen_cycle() {
        let mut t = open_todo();
        t.complete("alice").unwrap();
        assert_eq!(
            t.complete("alice").unwrap_err(),
            TodoError::AlreadyCompleted
        );
        t.reopen("alice").unwrap();
        assert_eq!(t.status, TodoStatus::Open);
        assert_eq!(t.reopen("alice").unwrap_err(), TodoError::NotCompleted);
    }

    #[test]
    fn rename_trims_and_rejects_empty() {
        let mut t = open_todo();
        t.rename("alice", "  Eggs  ").unwrap();
        assert_eq!(t.title, "Eggs");
        assert_eq!(
            t.rename("alice", "   ").unwrap_err(),
            TodoError::EmptyTitle
        );
        // same title is no-op (no extra version if no event — check version stays)
        let v = t.entity.version();
        t.rename("alice", "Eggs").unwrap();
        assert_eq!(t.entity.version(), v);
    }

    #[test]
    fn archive_is_terminal_for_mutations() {
        let mut t = open_todo();
        t.archive("alice").unwrap();
        assert_eq!(t.status, TodoStatus::Archived);
        assert_eq!(t.complete("alice").unwrap_err(), TodoError::Archived);
        assert_eq!(t.rename("alice", "x").unwrap_err(), TodoError::Archived);
        // second archive is idempotent
        t.archive("alice").unwrap();
        assert_eq!(t.status, TodoStatus::Archived);
    }
}
