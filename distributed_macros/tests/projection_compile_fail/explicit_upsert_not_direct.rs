use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed_macros::{projection, DomainEvent, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, DomainEvent)]
#[domain_event(name = "todo.created", version = 1)]
struct TodoCreated {
    todo_id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "todos", primary_key = ["todo_id"])]
struct Todos {
    todo_id: String,
}

const INVALID: ProjectionDescriptor<DirectCandidate> = projection! {
    name: "invalid";
    version: 1;
    epoch: "invalid-v1";
    partition: unit;
    on TodoCreated(event) {
        upsert Todos {
            key { todo_id: event.todo_id },
            set {}
        };
    }
};

fn main() {
    let _ = INVALID;
}
