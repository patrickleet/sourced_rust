use distributed::graphql::{PreparedCommand, Projected};
use distributed::GraphqlOutput;
use serde::Serialize;

#[derive(Serialize, GraphqlOutput)]
struct Output {
    id: String,
}

fn main() {
    let _ = PreparedCommand::<Projected<Output>>::prepare(Output { id: "one".into() });
}
