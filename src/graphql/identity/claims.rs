//! JWT claims → Session mapping (spec D4/D5, fixtures F1/F2).

use serde_json::Value;

use crate::microsvc::{Session, ROLE_KEY, USER_ID_KEY};

/// Configuration for claim → Session mapping.
#[derive(Debug, Clone)]
pub struct ClaimMapConfig {
    /// JWT claim for subject → `x-user-id`.
    pub subject_claim: String,
    /// Ordered claim paths for role candidates.
    pub role_claims: Vec<String>,
    /// Priority list for selecting `x-role` (first match wins).
    pub role_priority: Vec<String>,
    /// Engine-configured role names (schema keys). Empty = accept any candidate.
    pub engine_roles: Vec<String>,
}

impl Default for ClaimMapConfig {
    fn default() -> Self {
        Self {
            subject_claim: "sub".into(),
            role_claims: vec![
                "urn:zitadel:iam:org:project:roles".into(),
                "groups".into(),
                "roles".into(),
            ],
            role_priority: vec![
                "admin".into(),
                "operator".into(),
                "customer".into(),
                "user".into(),
            ],
            engine_roles: Vec::new(),
        }
    }
}

/// Map validated JWT claims JSON to a Session.
///
/// Object role claims: keys sorted lexicographically (D5).
/// `x-roles`: comma-separated allowlisted candidates in first-seen order.
/// `x-role`: priority ∩ candidates (D4).
pub fn map_claims_to_session(claims: &Value, config: &ClaimMapConfig) -> Result<Session, String> {
    let sub = claims
        .get(&config.subject_claim)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("missing non-empty subject claim `{}`", config.subject_claim))?;

    let mut candidates: Vec<String> = Vec::new();
    for path in &config.role_claims {
        if let Some(node) = claim_path(claims, path) {
            collect_role_candidates(node, &mut candidates);
        }
    }

    // Intersect with engine roles when configured.
    let allowlisted: Vec<String> = if config.engine_roles.is_empty() {
        candidates.clone()
    } else {
        candidates
            .into_iter()
            .filter(|c| config.engine_roles.iter().any(|e| e == c))
            .collect()
    };

    let x_roles = allowlisted.join(",");
    let x_role = select_role(&allowlisted, &config.role_priority);

    let mut session = Session::new();
    session.set(USER_ID_KEY, sub);
    if let Some(role) = x_role {
        session.set(ROLE_KEY, role);
    }
    if !x_roles.is_empty() {
        session.set("x-roles", x_roles);
    }

    // Optional standard mappings when present.
    if let Some(email) = claims.get("email").and_then(|v| v.as_str()) {
        session.set("x-email", email);
    }
    if let Some(org) = claims
        .get("org_id")
        .and_then(|v| v.as_str())
        .or_else(|| claims.get("orgId").and_then(|v| v.as_str()))
    {
        session.set("x-org-id", org);
    }

    Ok(session)
}

fn claim_path<'a>(claims: &'a Value, path: &str) -> Option<&'a Value> {
    // Dotted path or single key (including URN keys with colons).
    if path.contains('.') && !path.starts_with("urn:") {
        let mut cur = claims;
        for part in path.split('.') {
            cur = cur.get(part)?;
        }
        Some(cur)
    } else {
        claims.get(path)
    }
}

fn collect_role_candidates(node: &Value, out: &mut Vec<String>) {
    match node {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                push_unique(out, k.clone());
            }
        }
        Value::Array(arr) => {
            for v in arr {
                if let Some(s) = v.as_str() {
                    push_unique(out, s.to_string());
                }
            }
        }
        Value::String(s) => push_unique(out, s.clone()),
        _ => {}
    }
}

fn push_unique(out: &mut Vec<String>, s: String) {
    if !out.iter().any(|e| e == &s) {
        out.push(s);
    }
}

fn select_role(allowlisted: &[String], priority: &[String]) -> Option<String> {
    if allowlisted.is_empty() {
        return None;
    }
    for p in priority {
        if allowlisted.iter().any(|a| a == p) {
            return Some(p.clone());
        }
    }
    Some(allowlisted[0].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cfg_with_engine(roles: &[&str]) -> ClaimMapConfig {
        ClaimMapConfig {
            engine_roles: roles.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn f1_zitadel_project_roles() {
        let claims = json!({
            "iss": "http://localhost:8080",
            "aud": ["graphql-api", "123@graphql"],
            "sub": "user-a-001",
            "exp": 4102444800_i64,
            "urn:zitadel:iam:org:project:roles": {
                "customer": { "280664559058878577": "zitadel.localhost" },
                "admin": { "280664559058878577": "zitadel.localhost" }
            }
        });
        let session = map_claims_to_session(&claims, &cfg_with_engine(&["admin", "customer", "user"]))
            .unwrap();
        assert_eq!(session.user_id(), Some("user-a-001"));
        assert_eq!(session.role(), Some("admin"));
        assert_eq!(session.get("x-roles"), Some("admin,customer"));
    }

    #[test]
    fn f2_groups_array() {
        let claims = json!({
            "iss": "http://localhost:8080",
            "aud": "graphql-api",
            "sub": "user-b-002",
            "exp": 4102444800_i64,
            "groups": ["customer", "other-unmapped"]
        });
        let session =
            map_claims_to_session(&claims, &cfg_with_engine(&["admin", "customer", "user"]))
                .unwrap();
        assert_eq!(session.user_id(), Some("user-b-002"));
        assert_eq!(session.role(), Some("customer"));
        assert_eq!(session.get("x-roles"), Some("customer"));
    }
}
