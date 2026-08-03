struct MismatchAggregate;
struct ExpectedInput;
struct ActualInput;

#[distributed::command(
    id = "todo.mismatch",
    input = ExpectedInput,
    outcome = distributed::graphql::Succeeded<ActualInput>
)]
async fn declared_type_mismatch(
    _context: &distributed::microsvc::CausalCommandContext<'_, MismatchAggregate>,
    _input: ActualInput,
) -> Result<
    distributed::graphql::PreparedCommand<distributed::graphql::Succeeded<ActualInput>>,
    distributed::microsvc::HandlerError,
> {
    unreachable!()
}

fn main() {}
