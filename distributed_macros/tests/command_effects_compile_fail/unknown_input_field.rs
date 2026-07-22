use distributed::{command_effects, GraphqlInput, ReadModel};

#[derive(GraphqlInput)]
struct Input {
    view_id: String,
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "views", primary_key = ["view_id"])]
struct View {
    view_id: String,
}

fn main() {
    let _ = command_effects! {
        input: Input;
        delete View { key { view_id: input.missing } };
    };
}
