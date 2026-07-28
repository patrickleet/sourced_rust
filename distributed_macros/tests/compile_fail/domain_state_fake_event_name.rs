use serde::Serialize;

#[derive(Serialize, distributed_macros::DomainState)]
#[domain_state(name = "todo.state", version = 1)]
struct TodoState {
    todo_id: String,
}

fn main() {}
