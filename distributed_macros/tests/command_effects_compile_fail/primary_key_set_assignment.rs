use distributed::{command_effects, GraphqlInput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, GraphqlInput)]
struct Input {
    id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "views", primary_key = ["id"])]
struct View {
    id: String,
}

fn main() {
    let _ = command_effects! {
        input: Input;
        upsert View {
            key { id: input.id },
            set { id: input.id }
        };
    };
}
