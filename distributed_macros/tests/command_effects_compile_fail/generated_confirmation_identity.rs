use distributed::graphql::SurfaceProjector;
use distributed::{command_confirmations, GraphqlInput, ReadModel};
use serde::{Deserialize, Serialize};

#[derive(GraphqlInput)]
struct Input {
    id: String,
}

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "views", primary_key = ["id"])]
struct View {
    id: String,
}

fn main() {
    let projector = SurfaceProjector::new("views")
        .facts(["view.changed"])
        .models(["View"]);
    let _ = command_confirmations! {
        input: Input;
        confirm projector -> View { key { id: uuid_v7() } };
    };
}
