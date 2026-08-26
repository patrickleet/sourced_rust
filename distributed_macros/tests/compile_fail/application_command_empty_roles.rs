struct Aggregate;
struct Input;

#[distributed::command(
    id = "todo.create",
    roles(),
    input = Input,
    outcome = distributed::graphql::Succeeded<Input>
)]
async fn empty_roles(
    _context: &distributed::microsvc::CausalCommandContext<'_, Aggregate>,
    _input: Input,
) -> Result<
    distributed::graphql::PreparedCommand<distributed::graphql::Succeeded<Input>>,
    distributed::microsvc::HandlerError,
> {
    unreachable!()
}

fn main() {}
