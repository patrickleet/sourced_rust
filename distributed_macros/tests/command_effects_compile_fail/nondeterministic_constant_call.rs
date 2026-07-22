use distributed::{command_effects, GraphqlInput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(GraphqlInput)]
struct Input {
    id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "views", primary_key = ["id"])]
struct View {
    id: String,
    count: i64,
}

fn count() -> i64 {
    1
}

fn main() {
    let _ = command_effects! {
        input: Input;
        patch View { key { id: input.id }, set { count: constant(count()) } };
    };
}
