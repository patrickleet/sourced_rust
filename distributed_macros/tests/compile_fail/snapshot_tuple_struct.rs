use distributed::Snapshot;

// The generated snapshot struct mirrors the aggregate's named fields, so
// tuple structs are unsupported.
#[derive(Snapshot)]
struct Todo(String);

fn main() {}
