use distributed::{command_effects, GraphqlInput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, GraphqlInput)]
struct Input {
    id: String,
    bytes: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "views", primary_key = ["id"])]
struct View {
    id: String,
    bytes: Vec<u8>,
}

fn main() {
    let _ = command_effects! {
        input: Input;
        patch View {
            key { id: input.id },
            set { bytes: input.bytes }
        };
    };
}
