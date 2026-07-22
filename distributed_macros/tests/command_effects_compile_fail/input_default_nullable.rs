use distributed::{command_input_defaults, GraphqlInput};
use serde::Deserialize;

#[derive(Deserialize, GraphqlInput)]
struct Input {
    id: Option<String>,
}

fn main() {
    let _ = command_input_defaults! {
        input: Input;
        default input.id = uuid_v7();
    };
}
