//! Projected read-model views for e2e-ui.

pub mod auth_user_view;
pub mod blob_game_view;
pub mod chat_message_view;
pub mod todo_view;

pub use auth_user_view::{
    map_zitadel_user_status, map_zitadel_user_upsert, AuthUserView, ZitadelEmail,
    ZitadelUserPayload,
};
pub use blob_game_view::{map_blob_fact, BlobGameView};
pub use chat_message_view::{map_chat_posted, ChatMessageView};
pub use todo_view::{map_todo_fact, TodoView};
