use distributed::microsvc::{CausalCommandContext, HandlerError};
use distributed::Aggregate;

pub(super) fn rejected(err: impl std::fmt::Display) -> HandlerError {
    HandlerError::Rejected(err.to_string())
}

pub(super) fn principal<A>(ctx: &CausalCommandContext<'_, A>) -> Result<String, HandlerError>
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.user_id().map(str::to_string)
}

pub(super) fn authenticated_user<A>(ctx: &CausalCommandContext<'_, A>) -> bool
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.session().user_id().is_some_and(|id| !id.is_empty())
}

pub(super) fn admin_user<A>(ctx: &CausalCommandContext<'_, A>) -> bool
where
    A: Aggregate + Send + Sync + 'static,
{
    authenticated_user(ctx) && ctx.session().has_role("admin")
}
