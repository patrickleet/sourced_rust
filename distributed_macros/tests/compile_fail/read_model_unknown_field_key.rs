use distributed::ReadModel;

#[derive(ReadModel)]
struct CounterView {
    id: String,
    // Typo: `indexd` is not a recognized readmodel field attribute key.
    #[readmodel(indexd)]
    value: i32,
}

fn main() {}
