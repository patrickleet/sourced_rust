//! Portable Chat command declarations.
//!
//! Zitadel ingest stays on the service module — not a cell class method.

use distributed::graphql::{Eventual, PreparedCommand};
use distributed::microsvc::{
    CausalCommandContext, CausalRouteDependencies, HandlerError, PortableCommand, Routes,
};
use distributed::Aggregate;
use serde::{Deserialize, Serialize};

use crate::domain_commands;
use crate::{ChatMessage, ChatMessagePostedDomainEvent, ChatMessageState};

fn rejected(err: impl std::fmt::Display) -> HandlerError {
    HandlerError::Rejected(err.to_string())
}

fn principal<A>(ctx: &CausalCommandContext<'_, A>) -> Result<String, HandlerError>
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.user_id().map(str::to_string)
}

fn authenticated_user<A>(ctx: &CausalCommandContext<'_, A>) -> bool
where
    A: Aggregate + Send + Sync + 'static,
{
    ctx.session().user_id().is_some_and(|id| !id.is_empty())
}

/// `chat.post`
pub struct Post;

pub fn post() -> Post {
    Post
}

impl<D> PortableCommand<D> for Post
where
    D: CausalRouteDependencies<Aggregate = ChatMessage> + Send + Sync + 'static,
{
    fn install(self, routes: Routes<D>) -> Routes<D> {
        install_post(routes)
    }
}

impl Post {
    pub const COMMAND: &'static str = "chat.post";

    pub fn shard(input: &ChatPostInput) -> String {
        input.message_id.clone()
    }
}

#[derive(Debug, Deserialize, distributed::GraphqlInput)]
pub struct ChatPostInput {
    pub message_id: String,
    pub body: String,
    pub room_id: String,
    /// Client-generated unix milliseconds used by the optimistic row and
    /// accepted only when it is close to server time.
    pub created_at: String,
}

#[derive(Debug, Serialize, distributed::GraphqlOutput)]
pub struct ChatPostPayload {
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
}

pub async fn handle_post(
    ctx: &CausalCommandContext<'_, ChatMessage>,
    input: ChatPostInput,
) -> Result<PreparedCommand<Eventual<ChatPostPayload>>, HandlerError> {
    let author = principal(ctx)?;
    let created_at = canonical_near_unix_millis(&input.created_at)?;
    let repo = ctx.repo();

    if repo.get(&input.message_id).await?.is_some() {
        return Err(HandlerError::Rejected(format!(
            "message {} already exists",
            input.message_id
        )));
    }

    let mut msg = repo.create();
    msg.post(
        &input.message_id,
        &input.room_id,
        &author,
        &input.body,
        &created_at,
    )
    .map_err(rejected)?;

    let state = ChatMessageState::from(&*msg);
    repo.publish_events()
        .commit(msg)?
        .eventual(ChatPostPayload {
            message_id: state.message_id,
            room_id: state.room_id,
            author_id: state.author_id,
            body: state.body,
            created_at: state.created_at,
        })
}

/// Accept a client timestamp only when it is canonical unix milliseconds
/// within five minutes of server time.
pub fn canonical_near_unix_millis(value: &str) -> Result<String, HandlerError> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let millis = value
        .parse::<u128>()
        .map_err(|_| rejected("created_at must be canonical unix milliseconds"))?;
    if millis.to_string() != value || millis.abs_diff(now) > 300_000 {
        return Err(rejected(
            "created_at must be canonical unix milliseconds within five minutes of server time",
        ));
    }
    Ok(value.to_string())
}

fn install_post<D>(routes: Routes<D>) -> Routes<D>
where
    D: CausalRouteDependencies<Aggregate = ChatMessage> + Send + Sync + 'static,
{
    routes
        .command_transition::<domain_commands::Post, ChatPostInput, Eventual<ChatPostPayload>>(
            Post::COMMAND,
        )
        .field_name("chat_messages_post")
        .roles(["user", "admin"].into_iter())
        .authenticated_user_field::<ChatMessagePostedDomainEvent, ChatMessageState>("author_id")
        .guarded(authenticated_user, handle_post)
}

#[cfg(test)]
mod tests {
    use super::*;
    use distributed::{AggregateBuilder, InMemoryRepository};

    #[test]
    fn post_shard_is_message_id() {
        let input = ChatPostInput {
            message_id: "m1".into(),
            body: "hi".into(),
            room_id: "lobby".into(),
            created_at: "1".into(),
        };
        assert_eq!(Post::shard(&input), "m1");
    }

    #[test]
    fn post_handle_is_the_escape_hatch() {
        assert_eq!(Post::COMMAND, "chat.post");
        let _ = handle_post;
        let _ = canonical_near_unix_millis;
    }

    #[test]
    fn created_at_rejects_non_canonical() {
        assert!(canonical_near_unix_millis("not-a-time").is_err());
        assert!(canonical_near_unix_millis("01").is_err());
    }

    #[test]
    fn domain_declaration_mounts_without_sqlx_or_celld() {
        let repository = InMemoryRepository::new();
        let specs = Routes::new()
            .with_repo(repository.aggregate::<ChatMessage>())
            .mount(post())
            .command_specs()
            .expect("chat command declaration compiles");
        assert!(specs.iter().any(|spec| spec.id == "chat.post"));
        assert_eq!(
            specs
                .iter()
                .find(|spec| spec.id == "chat.post")
                .map(|spec| spec.field_name.as_str()),
            Some("chat_messages_post")
        );
    }
}
