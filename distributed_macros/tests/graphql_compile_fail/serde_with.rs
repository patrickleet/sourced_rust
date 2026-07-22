use distributed::GraphqlOutput;

#[derive(GraphqlOutput)]
struct CustomOutput {
    #[serde(with = "wire_value")]
    value: String,
}

fn main() {}
