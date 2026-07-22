use distributed::graphql::Accepted;

fn main() {
    let _ = Accepted {
        payload: String::from("not committed"),
    };
}
