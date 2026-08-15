//! Portable module contracts for pool-free GraphQL/client compilation.
//!
//! These declarations reuse the same typed command transitions as the
//! executable mounts but never construct a repository or `Service`.

use blob_domain::domain_commands as blob_commands;
use chat_domain::domain_commands as chat_commands;
use chat_domain::{ChatMessagePostedDomainEvent, ChatMessageState};
use distributed::application::{CommandDefinition, Module};
use distributed::command_input_defaults;
use distributed::graphql::{command_transition, Atomic, Eventual};
use e2e_readmodels::BlobGames;
use todo_domain::domain_commands as todo_commands;

use crate::handlers::commands::{
    blob_move, blob_start, blob_start_level, chat_post, payloads, todo_archive, todo_complete,
    todo_create, todo_force_archive, todo_purge, todo_rename, todo_reopen,
};

fn definition<I, K>(
    command: distributed::graphql::TypedCommand<I, K>,
) -> CommandDefinition
where
    I: distributed::graphql::GraphqlInputType + serde::de::DeserializeOwned + Send + 'static,
    K: distributed::graphql::CommandOutcome,
{
    CommandDefinition::from_typed_command(command, None)
        .expect("e2e contract command should compile without a mount")
}

/// Todo command contracts independent of process placement.
pub fn todo_module() -> Module {
    Module::new(super::todo::MODULE_ID)
        .command_definitions([
            definition(
                command_transition::<
                    todo_commands::Create,
                    todo_create::TodoCreateInput,
                    Eventual<todo_create::TodoCreatePayload>,
                >(todo_create::COMMAND)
                .field_name("todos_create")
                .roles(["user", "admin"])
                .input_defaults(command_input_defaults! {
                    input: todo_create::TodoCreateInput;
                    default input.todo_id = uuid_v7();
                }),
            ),
            definition(
                command_transition::<
                    todo_commands::Rename,
                    todo_rename::TodoRenameInput,
                    Eventual<todo_rename::TodoRenamePayload>,
                >(todo_rename::COMMAND)
                .field_name("todos_rename")
                .roles(["user", "admin"]),
            ),
            definition(
                command_transition::<
                    todo_commands::Complete,
                    todo_complete::TodoCompleteInput,
                    Eventual<payloads::TodoStatusPayload>,
                >(todo_complete::COMMAND)
                .field_name("todos_complete")
                .roles(["user", "admin"]),
            ),
            definition(
                command_transition::<
                    todo_commands::Reopen,
                    todo_reopen::TodoReopenInput,
                    Eventual<todo_reopen::TodoReopenPayload>,
                >(todo_reopen::COMMAND)
                .field_name("todos_reopen")
                .roles(["user", "admin"]),
            ),
            definition(
                command_transition::<
                    todo_commands::Archive,
                    todo_archive::TodoArchiveInput,
                    Eventual<todo_archive::TodoArchivePayload>,
                >(todo_archive::COMMAND)
                .field_name("todos_archive")
                .roles(["user", "admin"]),
            ),
            definition(
                command_transition::<
                    todo_commands::ForceArchive,
                    todo_force_archive::TodoForceArchiveInput,
                    Eventual<todo_force_archive::TodoForceArchivePayload>,
                >(todo_force_archive::COMMAND)
                .field_name("todos_force_archive")
                .roles(["admin"]),
            ),
            definition(
                command_transition::<
                    todo_commands::Purge,
                    todo_purge::TodoPurgeInput,
                    Eventual<todo_purge::TodoPurgePayload>,
                >(todo_purge::COMMAND)
                .field_name("todos_purge")
                .roles(["user", "admin"]),
            ),
        ])
        .build()
        .expect("todo contract module")
}

/// Chat command contracts independent of process placement.
pub fn chat_module() -> Module {
    Module::new(super::chat::MODULE_ID)
        .command_definitions([definition(
            command_transition::<
                chat_commands::Post,
                chat_post::ChatPostInput,
                Eventual<chat_post::ChatPostPayload>,
            >(chat_post::COMMAND)
            .field_name("chat_messages_post")
            .roles(["user", "admin"])
            .authenticated_user_field::<ChatMessagePostedDomainEvent, ChatMessageState>(
                "author_id",
            ),
        )])
        .build()
        .expect("chat contract module")
}

/// Blob Atomic command contracts independent of process placement.
pub fn blob_module() -> Module {
    Module::new(super::blob::MODULE_ID)
        .command_definitions([
            definition(
                command_transition::<
                    blob_commands::StartWithMap,
                    blob_start::BlobStartInput,
                    Atomic<BlobGames>,
                >(blob_start::COMMAND)
                .field_name("blob_games_start")
                .roles(["user", "admin"]),
            ),
            definition(
                command_transition::<
                    blob_commands::MoveDir,
                    blob_move::BlobMoveInput,
                    Atomic<BlobGames>,
                >(blob_move::COMMAND)
                .field_name("blob_games_move")
                .roles(["user", "admin"]),
            ),
            definition(
                command_transition::<
                    blob_commands::StartLevel,
                    blob_start_level::BlobStartLevelInput,
                    Atomic<BlobGames>,
                >(blob_start_level::COMMAND)
                .field_name("blob_games_start_level")
                .roles(["user", "admin"]),
            ),
        ])
        .build()
        .expect("blob contract module")
}

/// All portable e2e-ui modules used by client/schema compilation.
pub fn application_modules() -> Vec<Module> {
    vec![todo_module(), chat_module(), blob_module()]
}
