struct Aggregate;
struct Input;

#[distributed::command(
    id = "../Admin",
    roles(admin),
    input = Input,
    outcome = distributed::graphql::Succeeded<Input>
)]
async fn invalid_id(
    _context: &distributed::microsvc::CausalCommandContext<'_, Aggregate>,
    _input: Input,
) -> Result<
    distributed::graphql::PreparedCommand<distributed::graphql::Succeeded<Input>>,
    distributed::microsvc::HandlerError,
> {
    unreachable!()
}

fn main() {}
