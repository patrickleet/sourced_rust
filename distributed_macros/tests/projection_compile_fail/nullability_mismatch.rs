use distributed::projection::lower::{DirectCandidate, ProjectionDescriptor};
use distributed_macros::{projection, DomainState, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, DomainState)]
#[domain_state(version = 1)]
struct NullableState {
    todo_id: Option<String>,
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
    on "todo.changed" version 1 (state: NullableState) {
        upsert Todos from state;
    }
};

fn main() {
    let _ = INVALID;
}
