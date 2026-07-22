use distributed::{command_input_defaults, GraphqlInput};

#[derive(GraphqlInput)]
struct Input {
    id: String,
}

fn main() {
    let _ = command_input_defaults! {
        input: Input;
        default input.id = uuid_v7();
        default input.id = ulid();
    };
}
