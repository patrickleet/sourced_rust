//! Auth users imported from Zitadel Action ingress (provider messages → projector).

use distributed::ReadModel;
use serde::{Deserialize, Serialize};

use blob_domain::BlobGames;
use chat_domain::ChatMessages;

/// Imported IdP user. PK: `user_id` (= Zitadel subject / OIDC `sub` / session `x-user-id`).
///
/// Populated only from the Zitadel ingestor projector — never from commands.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ReadModel)]
#[table("auth_users")]
pub struct AuthUserView {
    #[id("user_id")]
    pub user_id: String,
    pub email: String,
    pub display_name: String,
    /// `human` | `machine`
    pub user_kind: String,
    /// `pending` | `approved` | `rejected`
    pub approval_status: String,
    /// `active` | `deactivated`
    pub status: String,
    pub updated_at: String,
    #[readmodel(has_many = "ChatMessages", foreign_key = "author_id")]
    pub chat_messages: Vec<ChatMessages>,
    #[readmodel(has_many = "BlobGames", foreign_key = "owner_id")]
    pub blob_games: Vec<BlobGames>,
}

/// Provider envelope published by `zitadel.ingress.v1` (fixture + Action shape).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZitadelUserPayload {
    pub schema_version: u32,
    pub source: String,
    pub delivery_id: String,
    pub provider: String,
    pub provider_subject: String,
    pub user_kind: String,
    pub emails: Vec<ZitadelEmail>,
    pub display_name: Option<String>,
    pub approval_status: String,
    pub ingested_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZitadelEmail {
    pub address: String,
    pub primary: bool,
    pub verified: bool,
}

/// Map a created/updated provider payload into an upsert row.
pub fn map_zitadel_user_upsert(event_name: &str, p: &ZitadelUserPayload) -> AuthUserView {
    let email = p
        .emails
        .iter()
        .find(|e| e.primary)
        .or_else(|| p.emails.first())
        .map(|e| e.address.clone())
        .unwrap_or_default();
    let display_name = p
        .display_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if email.is_empty() {
                p.provider_subject.clone()
            } else {
                email.clone()
            }
        });
    let status = if event_name.contains("deactivated") {
        "deactivated".into()
    } else {
        "active".into()
    };
    AuthUserView {
        user_id: p.provider_subject.clone(),
        email,
        display_name,
        user_kind: p.user_kind.clone(),
        approval_status: p.approval_status.clone(),
        status,
        updated_at: p.ingested_at.clone(),
        chat_messages: Vec::new(),
        blob_games: Vec::new(),
    }
}

/// Status-only patch for deactivate / reactivate when no profile fields are present.
pub fn map_zitadel_user_status(event_name: &str, p: &ZitadelUserPayload) -> AuthUserView {
    let mut row = map_zitadel_user_upsert(event_name, p);
    if event_name.contains("reactivated") {
        row.status = "active".into();
    } else if event_name.contains("deactivated") {
        row.status = "deactivated".into();
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> ZitadelUserPayload {
        ZitadelUserPayload {
            schema_version: 1,
            source: "zitadel".into(),
            delivery_id: "d1".into(),
            provider: "zitadel".into(),
            provider_subject: "user-99".into(),
            user_kind: "human".into(),
            emails: vec![ZitadelEmail {
                address: "ada@example.com".into(),
                primary: true,
                verified: true,
            }],
            display_name: Some("Ada".into()),
            approval_status: "pending".into(),
            ingested_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn created_maps_active_user() {
        let row = map_zitadel_user_upsert("zitadel.user.human.created.v1", &payload());
        assert_eq!(row.user_id, "user-99");
        assert_eq!(row.email, "ada@example.com");
        assert_eq!(row.display_name, "Ada");
        assert_eq!(row.status, "active");
        assert_eq!(row.approval_status, "pending");
    }

    #[test]
    fn deactivated_sets_status() {
        let row = map_zitadel_user_status("zitadel.user.human.deactivated.v1", &payload());
        assert_eq!(row.status, "deactivated");
        assert_eq!(row.user_id, "user-99");
    }
}
