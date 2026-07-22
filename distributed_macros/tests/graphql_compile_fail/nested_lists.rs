use distributed::GraphqlInput;

#[derive(GraphqlInput)]
struct NestedLists {
    values: Option<Vec<Option<Vec<String>>>>,
}

fn main() {}
