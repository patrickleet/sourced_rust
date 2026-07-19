//! Projected read-model views for e2e-ui.

mod auth_user_view;
mod blob_game_view;
mod chat_message_view;
mod todo_view;

pub use auth_user_view::{
    map_zitadel_user_status, map_zitadel_user_upsert, AuthUserView, ZitadelEmail, ZitadelUserPayload,
};
pub use blob_game_view::{map_blob_fact, BlobGameView};
pub use chat_message_view::{map_chat_posted, ChatMessageView};
pub use todo_view::{map_todo_fact, TodoView};

// Back-compat alias used by todo projector.
pub use map_todo_fact as map_fact;
