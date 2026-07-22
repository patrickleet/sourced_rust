use distributed::{command_effects, GraphqlInput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(GraphqlInput)]
struct Input {
    id: String,
    title: Option<String>,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "views", primary_key = ["id"])]
struct View {
    id: String,
    title: String,
}

fn main() {
    let _ = command_effects! {
        input: Input;
        patch View { key { id: input.id }, set { title: input.title } };
    };
}
