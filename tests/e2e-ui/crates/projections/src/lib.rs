//! Event-to-read-model programs for the e2e-ui CQRS fixture.
//!
//! Domain crates own aggregate behavior and outward event contracts.
//! `e2e-readmodels` owns query shapes and read authorization. This crate owns
//! the reusable mapping between those two sides.

mod auth_users;
mod blob;
mod chat;
mod todos;

pub use auth_users::{
    map_zitadel_user_status, map_zitadel_user_upsert, ZitadelEmail, ZitadelUserPayload,
};
pub use blob::{
    blob_mutation_projection_program, save_blob_game, save_blob_game_program,
    BlobDirectEligibilityGuards, BLOB_GAMES,
};
pub use chat::{
    chat_mutation_projection_program, save_chat_message, save_chat_message_program, CHAT_MESSAGES,
};
pub use todos::{
    complete_preview, delete_todo, delete_todo_program, save_todo, save_todo_program,
    todo_mutation_projection_program, TODO_READS,
};
