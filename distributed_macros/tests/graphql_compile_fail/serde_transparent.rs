use distributed::GraphqlInput;

#[derive(GraphqlInput)]
#[serde(transparent)]
struct TransparentInput {
    value: String,
}

fn main() {}
