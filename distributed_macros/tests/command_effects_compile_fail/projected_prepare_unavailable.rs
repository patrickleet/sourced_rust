use distributed::graphql::{PreparedCommand, Projected};
use distributed::ReadModel;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, ReadModel)]
#[readmodel(table = "outputs", primary_key = ["id"])]
struct Output {
    id: String,
}

fn main() {
    let _ = PreparedCommand::<Projected<Output>>::prepare(Output { id: "one".into() });
}
