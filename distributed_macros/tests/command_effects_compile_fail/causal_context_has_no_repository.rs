use distributed::microsvc::CausalCommandContext;

fn handler(context: &CausalCommandContext<'_>) {
    let _ = context.repo();
    let _ = context.dependencies();
    let _ = context.read_model_store();
}

fn main() {}
