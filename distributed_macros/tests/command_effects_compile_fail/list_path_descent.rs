use distributed::{command_effects, GraphqlInput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(GraphqlInput)]
struct ItemInput {
    title: String,
}

#[derive(GraphqlInput)]
struct Input {
    id: String,
    items: Vec<ItemInput>,
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
        patch View {
            key { id: input.id },
            set { title: input.items<ItemInput>.title }
        };
    };
}
