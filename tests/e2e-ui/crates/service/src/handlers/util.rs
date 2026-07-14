//! Shared handler helpers.

use distributed::bus::Message;
use distributed::microsvc::{HandlerError, Session};
use distributed::{BitcodePayloadCodec, PayloadCodec};
use serde::de::DeserializeOwned;

/// Decode event payload as JSON (tests) or bitcode (outbox → bus).
pub fn decode_payload<T: DeserializeOwned>(message: &Message) -> Result<T, HandlerError> {
    let ct = message.content_type.as_str();
    if ct.contains("json") || looks_like_json(message.payload()) {
        return serde_json::from_slice(message.payload())
            .map_err(|e| HandlerError::DecodeFailed(format!("json payload: {e}")));
    }
    BitcodePayloadCodec::decode(message.payload())
        .map_err(|e| HandlerError::DecodeFailed(format!("bitcode payload: {e}")))
}

fn looks_like_json(bytes: &[u8]) -> bool {
    matches!(
        bytes.iter().find(|b| !b.is_ascii_whitespace()),
        Some(b'{' | b'[')
    )
}

pub fn rejected(err: impl std::fmt::Display) -> HandlerError {
    HandlerError::Rejected(err.to_string())
}

pub fn read_model_error(e: impl std::fmt::Display) -> HandlerError {
    HandlerError::Other(Box::new(std::io::Error::other(e.to_string())))
}

/// Authenticated user from session (`x-user-id` via DevHeaders or OIDC claim map).
pub fn require_user(session: &Session) -> Result<String, HandlerError> {
    session
        .user_id()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| HandlerError::Unauthorized("missing x-user-id".into()))
}

/// Session has a non-empty user id (for `guard` — bool, not Result).
pub fn session_has_user(session: &Session) -> bool {
    session.user_id().is_some_and(|s| !s.is_empty())
}

/// Engine role is `admin` (`x-role` / OIDC claim map). For `guard`.
pub fn session_is_admin(session: &Session) -> bool {
    session.role() == Some("admin")
}

/// Require engine role `admin` (handler-path Result form).
pub fn require_admin(session: &Session) -> Result<(), HandlerError> {
    match session.role() {
        Some("admin") => Ok(()),
        Some(other) => Err(HandlerError::Rejected(format!(
            "admin role required, got `{other}`"
        ))),
        None => Err(HandlerError::Unauthorized("missing x-role".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use distributed::microsvc::{Session, ROLE_KEY, USER_ID_KEY};

    #[test]
    fn session_has_user_requires_nonempty_id() {
        let mut s = Session::new();
        assert!(!session_has_user(&s));
        s.set(USER_ID_KEY, "");
        assert!(!session_has_user(&s));
        s.set(USER_ID_KEY, "alice");
        assert!(session_has_user(&s));
    }

    #[test]
    fn session_is_admin_exact_role() {
        let mut s = Session::new();
        assert!(!session_is_admin(&s));
        s.set(ROLE_KEY, "user");
        assert!(!session_is_admin(&s));
        s.set(ROLE_KEY, "admin");
        assert!(session_is_admin(&s));
    }

    #[test]
    fn require_admin_errors() {
        let mut s = Session::new();
        assert!(require_admin(&s).is_err());
        s.set(ROLE_KEY, "user");
        assert!(require_admin(&s).is_err());
        s.set(ROLE_KEY, "admin");
        assert!(require_admin(&s).is_ok());
    }

    #[test]
    fn require_user_errors_and_returns_id() {
        let mut s = Session::new();
        assert!(require_user(&s).is_err());
        s.set(USER_ID_KEY, "bob");
        assert_eq!(require_user(&s).unwrap(), "bob");
    }
}
