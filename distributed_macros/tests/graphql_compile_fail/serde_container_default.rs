use distributed::GraphqlInput;

#[derive(GraphqlInput)]
#[serde(default = "default_input")]
struct ContainerDefaultInput {
    value: String,
}

fn main() {}
