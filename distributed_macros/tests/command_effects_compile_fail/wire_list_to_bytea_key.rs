use distributed::{command_effects, GraphqlInput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, GraphqlInput)]
struct Input {
    bytes_id: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "views", primary_key = ["bytes_id"])]
struct View {
    id: String,
    bytes_id: Vec<u8>,
}

fn main() {
    let _ = command_effects! {
        input: Input;
        delete View { key { bytes_id: input.bytes_id } };
    };
}
