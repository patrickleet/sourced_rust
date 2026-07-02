use distributed::Snapshot;

// Snapshot needs the aggregate's entity field to source the snapshot id.
// There is no field named `entity` and no `#[snapshot(entity = "...")]`
// override pointing at an existing field.
#[derive(Snapshot)]
struct Todo {
    task: String,
}

fn main() {}
