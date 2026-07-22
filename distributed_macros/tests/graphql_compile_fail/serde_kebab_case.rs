use distributed::GraphqlOutput;

#[derive(GraphqlOutput)]
#[serde(rename_all = "kebab-case")]
struct KebabCase {
    field_name: String,
}

fn main() {}
