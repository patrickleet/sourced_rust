use distributed::ReadModel;
use serde::{Deserialize, Serialize};

/// Insert-shaped chat message row. The plural name infers `chat_messages`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[readmodel(primary_key = ["message_id"])]
pub struct ChatMessages {
    #[readmodel(id)]
    pub message_id: String,
    pub room_id: String,
    pub author_id: String,
    pub body: String,
    pub created_at: String,
}
