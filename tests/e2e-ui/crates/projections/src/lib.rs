//! Domain-event handlers over read-model mutations for the e2e-ui fixture.

mod auth_users;
mod blob;
mod chat;
mod todos;

pub use auth_users::{
    map_zitadel_user_status, map_zitadel_user_upsert, ZitadelEmail, ZitadelUserPayload,
};
pub use blob::{save_blob_game, BlobDirectEligibilityGuards, BLOB_GAMES};
pub use chat::{save_chat_message, CHAT_MESSAGES};
pub use todos::{delete_todo, save_todo, TODOS};
