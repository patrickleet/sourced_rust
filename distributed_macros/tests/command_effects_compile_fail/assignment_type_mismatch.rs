use distributed::{command_effects, GraphqlInput, ReadModel};

#[derive(GraphqlInput)]
struct Input {
    view_id: String,
    title: String,
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "views", primary_key = ["view_id"])]
struct View {
    view_id: String,
    count: i64,
}

fn main() {
    let _ = command_effects! {
        input: Input;
        patch View {
            key { view_id: input.view_id },
            set { count: input.title }
        };
    };
}
