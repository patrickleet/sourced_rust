//! Authenticity for Zitadel Action → HTTP deliveries.
//!
//! Paths:
//! 1. **Shared secret** header `x-zitadel-ingestor-secret` or `Authorization: Bearer`
//! 2. **Actions v2 event body** when `ZITADEL_INGESTOR_ALLOW_ACTION_EVENTS=1` (local only)

use std::env;

use distributed::microsvc::{HandlerError, Session};

/// Env var for the shared secret (required for fixture/curl path).
pub const SECRET_ENV: &str = "ZITADEL_INGESTOR_SECRET";

/// Preferred Action/HTTP header (lowercase session keys).
pub const SECRET_HEADER: &str = "x-zitadel-ingestor-secret";

/// When `1`/`true`, accept native Actions v2 event envelopes without shared secret.
pub const ALLOW_ACTION_EVENTS_ENV: &str = "ZITADEL_INGESTOR_ALLOW_ACTION_EVENTS";

pub fn configured_secret() -> Option<String> {
    env::var(SECRET_ENV).ok().filter(|s| !s.trim().is_empty())
}

pub fn allow_action_events() -> bool {
    matches!(
        env::var(ALLOW_ACTION_EVENTS_ENV)
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1") | Some("true") | Some("TRUE") | Some("yes")
    )
}

pub fn presented_secret(session: &Session) -> Option<String> {
    if let Some(v) = session.get(SECRET_HEADER).filter(|s| !s.is_empty()) {
        return Some(v.to_string());
    }
    if let Some(auth) = session.get("authorization") {
        if let Some(token) = auth
            .strip_prefix("Bearer ")
            .or_else(|| auth.strip_prefix("bearer "))
        {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

pub fn verify_authenticity(session: &Session, is_action_event: bool) -> Result<(), HandlerError> {
    if let Some(presented) = presented_secret(session) {
        let expected = configured_secret().ok_or_else(|| {
            HandlerError::Unauthorized(format!(
                "{SECRET_ENV} is not configured; refusing Zitadel ingress"
            ))
        })?;
        if presented != expected {
            return Err(HandlerError::Unauthorized(
                "invalid Zitadel ingestor secret".into(),
            ));
        }
        return Ok(());
    }

    if is_action_event && allow_action_events() {
        return Ok(());
    }

    if is_action_event {
        return Err(HandlerError::Unauthorized(format!(
            "Action event rejected: set {ALLOW_ACTION_EVENTS_ENV}=1 (local) or send {SECRET_HEADER}"
        )));
    }

    let _expected = configured_secret().ok_or_else(|| {
        HandlerError::Unauthorized(format!(
            "{SECRET_ENV} is not configured; refusing Zitadel ingress"
        ))
    })?;
    Err(HandlerError::Unauthorized(format!(
        "missing Zitadel authenticity ({SECRET_HEADER} or Authorization: Bearer)"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env(secret: Option<&str>, allow_actions: bool, f: impl FnOnce()) {
        let _g = ENV_LOCK.lock().unwrap();
        let prev_s = env::var(SECRET_ENV).ok();
        let prev_a = env::var(ALLOW_ACTION_EVENTS_ENV).ok();
        match secret {
            Some(s) => env::set_var(SECRET_ENV, s),
            None => env::remove_var(SECRET_ENV),
        }
        if allow_actions {
            env::set_var(ALLOW_ACTION_EVENTS_ENV, "1");
        } else {
            env::remove_var(ALLOW_ACTION_EVENTS_ENV);
        }
        f();
        match prev_s {
            Some(s) => env::set_var(SECRET_ENV, s),
            None => env::remove_var(SECRET_ENV),
        }
        match prev_a {
            Some(s) => env::set_var(ALLOW_ACTION_EVENTS_ENV, s),
            None => env::remove_var(ALLOW_ACTION_EVENTS_ENV),
        }
    }

    fn session(pairs: &[(&str, &str)]) -> Session {
        let mut m = HashMap::new();
        for (k, v) in pairs {
            m.insert((*k).to_string(), (*v).to_string());
        }
        Session::from_map(m)
    }

    #[test]
    fn rejects_when_secret_not_configured() {
        with_env(None, false, || {
            let err = verify_authenticity(&session(&[(SECRET_HEADER, "x")]), false).unwrap_err();
            assert!(matches!(err, HandlerError::Unauthorized(_)));
        });
    }

    #[test]
    fn accepts_matching_header() {
        with_env(Some("s3cret"), false, || {
            verify_authenticity(&session(&[(SECRET_HEADER, "s3cret")]), false).unwrap();
        });
    }

    #[test]
    fn accepts_action_event_when_allowed() {
        with_env(None, true, || {
            verify_authenticity(&Session::new(), true).unwrap();
        });
    }
}
