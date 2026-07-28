use distributed::graphql::Succeeded;

fn main() {
    let _ = Succeeded {
        payload: String::from("not committed"),
    };
}
