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
    title: String,
}

fn main() {
    let generated_at_runtime = String::from("not deterministic declaration IR");
    let _ = command_effects! {
        input: Input;
        patch View {
            key { id: input.id },
            set { title: constant(generated_at_runtime) }
        };
    };
}
