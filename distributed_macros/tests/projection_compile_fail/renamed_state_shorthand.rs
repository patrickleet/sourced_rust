use distributed::projection::lower::{DirectEligible, ProjectionDescriptor};
use distributed_macros::{projection, DomainState, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, DomainState)]
#[domain_state(version = 1)]
#[serde(rename_all = "camelCase")]
struct RenamedState {
    todo_id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "todos", primary_key = ["todo_id"])]
struct Todos {
    todo_id: String,
}

const INVALID: ProjectionDescriptor<DirectEligible> = projection! {
    name: "invalid";
    version: 1;
    epoch: "invalid-v1";
    partition: unit;
    on "todo.created" version 1 (state: RenamedState) {
        upsert Todos from state;
    }
};

fn main() {
    let _ = INVALID;
}
