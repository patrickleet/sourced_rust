use distributed::GraphqlOutput;

#[derive(GraphqlOutput)]
struct SkippedOutput {
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

fn main() {}
