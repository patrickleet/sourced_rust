use distributed::{command_effects, GraphqlInput, ReadModel};

#[derive(GraphqlInput)]
struct Input {
    tenant_id: String,
    record_id: String,
}

#[derive(Clone, ReadModel)]
#[readmodel(table = "views", primary_key = ["tenant_id", "record_id"])]
struct View {
    tenant_id: String,
    record_id: String,
}

fn main() {
    let _ = command_effects! {
        input: Input;
        delete View { key { record_id: input.record_id } };
    };
}
