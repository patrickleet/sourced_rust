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

    fn is_open(&self) -> bool {
        matches!(self.status, TodoStatus::Open)
    }

    fn is_completed(&self) -> bool {
        matches!(self.status, TodoStatus::Completed)
    }

    fn is_archived(&self) -> bool {
        matches!(self.status, TodoStatus::Archived)
    }
}

/// One public function per command. `#[event]` records history and applies state.
/// `when =` skips recording when the command must not fire (invalid / no-op).
#[sourced(entity, events = "TodoEvent", aggregate_type = "todo")]
impl Todo {
    /// Create a new open todo. Callers should pass trimmed, non-empty fields.
    #[event(
        "todo.created",
        when = !self.is_created()
            && !todo_id.trim().is_empty()
            && !owner_id.trim().is_empty()
            && !title.trim().is_empty()
    )]
    pub fn create(&mut self, todo_id: String, owner_id: String, title: String) {
        self.entity.set_id(&todo_id);
        self.todo_id = todo_id;
        self.owner_id = owner_id;
        self.title = title.trim().to_string();
        self.status = TodoStatus::Open;
    }

    /// Rename. No-ops when title is empty, unchanged, or todo is archived / not owned.
    #[event(
        "todo.renamed",
        when = self.owner_id == owner_id
            && !self.is_archived()
            && !title.trim().is_empty()
            && title.trim() != self.title
    )]
    pub fn rename(&mut self, owner_id: String, title: String) {
        self.title = title.trim().to_string();
    }

    /// Mark completed. No-ops unless open and owned by `owner_id`.
    #[event(
        "todo.completed",
        when = self.owner_id == owner_id && self.is_open()
    )]
    pub fn complete(&mut self, owner_id: String) {
        self.status = TodoStatus::Completed;
    }

    /// Reopen a completed todo. No-ops unless completed and owned by `owner_id`.
    #[event(
        "todo.reopened",
        when = self.owner_id == owner_id && self.is_completed()
    )]
    pub fn reopen(&mut self, owner_id: String) {
        self.status = TodoStatus::Open;
    }

    /// Archive. Idempotent: no-ops if already archived. No-ops if not owned.
    #[event(
        "todo.archived",
        when = self.owner_id == owner_id && self.is_created() && !self.is_archived()
    )]
    pub fn archive(&mut self, owner_id: String) {
        self.status = TodoStatus::Archived;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_todo() -> Todo {
        let mut t = Todo::default();
        let _ = t.create("t1".into(), "alice".into(), "Buy milk".into());
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
    fn create_noops_on_empty_or_duplicate() {
        let mut t = Todo::default();
        let _ = t.create("t1".into(), "alice".into(), "  ".into());
        assert!(!t.is_created());
        assert_eq!(t.entity.version(), 0);

        let _ = t.create("t1".into(), "alice".into(), "ok".into());
        assert!(t.is_created());
        let v = t.entity.version();
        let _ = t.create("t1".into(), "alice".into(), "again".into());
        assert_eq!(t.entity.version(), v);
        assert_eq!(t.title, "ok");
    }

    #[test]
    fn only_owner_can_mutate() {
        let mut t = open_todo();
        let _ = t.complete("bob".into());
        assert_eq!(t.status, TodoStatus::Open);
        assert_eq!(t.entity.version(), 1);

        let _ = t.complete("alice".into());
        assert_eq!(t.status, TodoStatus::Completed);
        assert_eq!(t.entity.version(), 2);
    }

    #[test]
    fn complete_reopen_cycle() {
        let mut t = open_todo();
        let _ = t.complete("alice".into());
        assert_eq!(t.status, TodoStatus::Completed);
        let v = t.entity.version();
        let _ = t.complete("alice".into());
        assert_eq!(t.entity.version(), v);

        let _ = t.reopen("alice".into());
        assert_eq!(t.status, TodoStatus::Open);
        let v = t.entity.version();
        let _ = t.reopen("alice".into());
        assert_eq!(t.entity.version(), v);
    }

    #[test]
    fn rename_trims_via_caller_and_noops_empty() {
        let mut t = open_todo();
        let _ = t.rename("alice".into(), "Eggs".into());
        assert_eq!(t.title, "Eggs");
        let v = t.entity.version();
        let _ = t.rename("alice".into(), String::new());
        assert_eq!(t.entity.version(), v);
        assert_eq!(t.title, "Eggs");
        let _ = t.rename("alice".into(), "Eggs".into());
        assert_eq!(t.entity.version(), v);
    }

    #[test]
    fn archive_is_terminal_for_mutations() {
        let mut t = open_todo();
        let _ = t.archive("alice".into());
        assert_eq!(t.status, TodoStatus::Archived);
        let v = t.entity.version();
        let _ = t.complete("alice".into());
        let _ = t.rename("alice".into(), "x".into());
        let _ = t.archive("alice".into());
        assert_eq!(t.entity.version(), v);
        assert_eq!(t.status, TodoStatus::Archived);
    }
}
