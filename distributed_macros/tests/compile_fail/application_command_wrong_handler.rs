struct WrongAggregate;
struct WrongInput;

#[distributed::command(
    id = "todo.create",
    input = WrongInput,
    outcome = distributed::graphql::Succeeded<WrongInput>
)]
async fn wrong_handler(
    _context: &distributed::microsvc::CausalCommandContext<'_, WrongAggregate>,
    _input: WrongInput,
) -> Result<(), distributed::microsvc::HandlerError> {
    Ok(())
}

fn main() {}
