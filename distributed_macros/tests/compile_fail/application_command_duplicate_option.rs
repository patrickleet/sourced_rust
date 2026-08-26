struct Aggregate;
struct Input;

#[distributed::command(
    id = "todo.create",
    id = "todo.rename",
    roles(user),
    input = Input,
    outcome = distributed::graphql::Succeeded<Input>
)]
async fn duplicate_option(
    _context: &distributed::microsvc::CausalCommandContext<'_, Aggregate>,
    _input: Input,
) -> Result<
    distributed::graphql::PreparedCommand<distributed::graphql::Succeeded<Input>>,
    distributed::microsvc::HandlerError,
> {
    unreachable!()
}

fn main() {}
