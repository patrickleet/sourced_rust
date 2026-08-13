use distributed::GraphqlInput;

#[derive(GraphqlInput)]
struct DefaultedInput {
    #[serde(default)]
    value: String,
}

fn main() {}
