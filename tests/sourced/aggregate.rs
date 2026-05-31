use distributed::{sourced, Entity};

#[derive(Default)]
pub struct Todo {
    pub entity: Entity,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}

#[sourced(entity)]
impl Todo {
    #[event("Initialized")]
    pub fn initialize(&mut self, id: String, user_id: String, task: String) {
        self.entity.set_id(&id);
        self.user_id = user_id;
        self.task = task;
    }

    #[event("Completed", when = !self.completed)]
    pub fn complete(&mut self) {
        self.completed = true;
    }

    #[event("TailValidated")]
    pub fn validate_tail(&mut self) {
        self.completed = true;
        self.tail_check()?
    }

    fn tail_check(&self) -> distributed::SourcedResult {
        Ok(())
    }

    // Non-event method should pass through unchanged
    pub fn snapshot(&self) -> TodoSnapshot {
        TodoSnapshot {
            id: self.entity.id().to_string(),
            user_id: self.user_id.clone(),
            task: self.task.clone(),
            completed: self.completed,
        }
    }
}

// Generated: TodoEvent enum, TryFrom<&EventRecord>, impl Aggregate

#[derive(Clone, Debug)]
pub struct TodoSnapshot {
    pub id: String,
    pub user_id: String,
    pub task: String,
    pub completed: bool,
}
