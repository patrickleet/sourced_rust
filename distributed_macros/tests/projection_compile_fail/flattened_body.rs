use distributed_macros::DomainEvent;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
struct Details {
    title: String,
}

#[derive(Clone, Serialize, Deserialize, DomainEvent)]
#[domain_event(name = "todo.changed", version = 1)]
struct Flattened {
    todo_id: String,
    #[serde(flatten)]
    details: Details,
}

fn main() {}
