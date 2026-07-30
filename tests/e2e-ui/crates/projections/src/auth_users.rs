//! Zitadel integration-event projection into the AuthUsers query model.

use e2e_readmodels::AuthUsers;
use serde::{Deserialize, Serialize};

/// Provider envelope published by `zitadel.ingress.v1`.
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

/// Map a created/updated provider event into an upsert row.
pub fn map_zitadel_user_upsert(event_name: &str, payload: &ZitadelUserPayload) -> AuthUsers {
    let email = payload
        .emails
        .iter()
        .find(|email| email.primary)
        .or_else(|| payload.emails.first())
        .map(|email| email.address.clone())
        .unwrap_or_default();
    let display_name = payload
        .display_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            if email.is_empty() {
                payload.provider_subject.clone()
            } else {
                email.clone()
            }
        });
    let status = if event_name.contains("deactivated") {
        "deactivated".into()
    } else {
        "active".into()
    };
    AuthUsers {
        user_id: payload.provider_subject.clone(),
        email,
        display_name,
        user_kind: payload.user_kind.clone(),
        approval_status: payload.approval_status.clone(),
        status,
        updated_at: payload.ingested_at.clone(),
    }
}

/// Map a deactivate/reactivate provider event into its current user row.
pub fn map_zitadel_user_status(event_name: &str, payload: &ZitadelUserPayload) -> AuthUsers {
    let mut row = map_zitadel_user_upsert(event_name, payload);
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
