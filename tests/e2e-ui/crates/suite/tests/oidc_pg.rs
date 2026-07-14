//! Live OIDC cell (gated by `E2E_STACK=1` + env from `scripts/up.sh`).
//! Soft-skips when unset so offline `make test` stays green.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde_json::{json, Value};

fn stack_enabled() -> bool {
    matches!(
        std::env::var("E2E_STACK")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes"
    )
}

async fn mint_machine_token(keyfile: &str, uid: &str, issuer: &str, project_id: &str) -> String {
    let raw = tokio::fs::read_to_string(keyfile).await.expect("keyfile");
    let v: Value = serde_json::from_str(&raw).expect("json");
    let kid = v["keyId"]
        .as_str()
        .or_else(|| v["key_id"].as_str())
        .expect("kid");
    let pem = v["key"].as_str().expect("pem");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let claims = json!({
        "iss": uid,
        "sub": uid,
        "aud": [format!("{issuer}/oauth/v2/token"), issuer],
        "iat": now,
        "exp": now + 120,
    });
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    let encoding = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("pem");
    let assertion = encode(&header, &claims, &encoding).expect("sign");

    let body = format!(
        "grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer&scope={}&assertion={}",
        urlencoding_lite(&format!(
            "openid profile urn:zitadel:iam:org:project:id:{project_id}:aud urn:zitadel:iam:org:project:roles"
        )),
        urlencoding_lite(&assertion)
    );
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{issuer}/oauth/v2/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .expect("token");
    assert!(
        resp.status().is_success(),
        "token mint {}",
        resp.status()
    );
    let j: Value = resp.json().await.expect("json");
    j["access_token"]
        .as_str()
        .expect("access_token")
        .to_string()
}

fn urlencoding_lite(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[tokio::test]
async fn oidc_bearer_graphql_isolation_against_stack() {
    if !stack_enabled() {
        eprintln!("E2E_STACK unset — skip live OIDC+Postgres cell");
        return;
    }
    let base = std::env::var("E2E_API_ORIGIN").unwrap_or_else(|_| "http://127.0.0.1:8791".into());
    let issuer = std::env::var("OIDC_ISSUER").expect("OIDC_ISSUER");
    let project = std::env::var("ZITADEL_PROJECT_ID").expect("ZITADEL_PROJECT_ID");
    let user_key = std::env::var("E2E_MACHINE_USER_KEY").expect("E2E_MACHINE_USER_KEY");
    let admin_key = std::env::var("E2E_MACHINE_ADMIN_KEY").expect("E2E_MACHINE_ADMIN_KEY");
    let user_uid = std::env::var("E2E_MACHINE_USER_ID").expect("E2E_MACHINE_USER_ID");
    let admin_uid = std::env::var("E2E_MACHINE_ADMIN_ID").expect("E2E_MACHINE_ADMIN_ID");

    let user_tok = mint_machine_token(&user_key, &user_uid, &issuer, &project).await;
    let admin_tok = mint_machine_token(&admin_key, &admin_uid, &issuer, &project).await;
    let client = reqwest::Client::new();

    let unauth = client
        .post(format!("{base}/graphql"))
        .header("content-type", "application/json")
        .json(&json!({"query":"{ todos { todo_id } }"}))
        .send()
        .await
        .expect("http");
    assert_eq!(unauth.status().as_u16(), 401, "unauth GraphQL must 401 under OidcBearer");

    // HTTP command routes are not mounted (GraphQL-only surface).
    let http_cmd = client
        .post(format!("{base}/todo.create"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {user_tok}"))
        .json(&json!({"todo_id": "t-http-off", "title": "nope"}))
        .send()
        .await
        .expect("http cmd");
    assert!(
        matches!(http_cmd.status().as_u16(), 404 | 405),
        "POST /todo.create must not be mounted, got {}",
        http_cmd.status()
    );

    // Mutation without Bearer but with spoofed identity headers must 401 (fail closed).
    let spoof_mut = client
        .post(format!("{base}/graphql"))
        .header("content-type", "application/json")
        .header("x-user-id", "spoofed-attacker")
        .header("x-role", "admin")
        .json(&json!({
            "query": r#"mutation { todos_create(input: { todo_id: "t-spoof", title: "no" }) { todo_id } }"#
        }))
        .send()
        .await
        .expect("spoof mut");
    assert_eq!(
        spoof_mut.status().as_u16(),
        401,
        "mutation with only spoof headers must 401, got body {}",
        spoof_mut.text().await.unwrap_or_default()
    );

    let tid = format!(
        "t-oidc-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
    );
    let create = client
        .post(format!("{base}/graphql"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {user_tok}"))
        .json(&json!({
            "query": format!(
                r#"mutation {{ todos_create(input: {{ todo_id: "{tid}", title: "OIDC todo" }}) {{ todo_id owner_id title status }} }}"#
            )
        }))
        .send()
        .await
        .expect("create");
    let create_body: serde_json::Value = create.json().await.expect("create json");
    assert!(
        create_body.get("errors").and_then(|e| e.as_array()).map(|a| a.is_empty()).unwrap_or(true),
        "create mutation errors: {create_body}"
    );
    assert_eq!(
        create_body["data"]["todos_create"]["todo_id"], tid,
        "{create_body}"
    );

    // Poll as the same user (owner-scoped) — proves projector + Bearer GraphQL.
    let mut seen = false;
    let mut last = String::new();
    for _ in 0..60 {
        let gql = client
            .post(format!("{base}/graphql"))
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {user_tok}"))
            .json(&json!({"query":"{ todos { todo_id owner_id title } }"}))
            .send()
            .await
            .expect("gql");
        assert!(
            gql.status().is_success(),
            "user gql HTTP {}",
            gql.status()
        );
        let body: Value = gql.json().await.unwrap();
        last = body.to_string();
        if let Some(arr) = body["data"]["todos"].as_array() {
            if arr.iter().any(|r| r["todo_id"] == tid) {
                seen = true;
                // owner is JWT sub, not spoofable
                for r in arr {
                    if r["todo_id"] == tid {
                        assert_eq!(r["owner_id"], user_uid);
                    }
                }
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(seen, "user should see projected todo; last={last}");

    // Spoof headers must not override Bearer subject.
    let spoof = client
        .post(format!("{base}/graphql"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {user_tok}"))
        .header("x-user-id", "evil")
        .header("x-role", "admin")
        .json(&json!({"query":"{ todos { todo_id owner_id } }"}))
        .send()
        .await
        .expect("spoof");
    assert!(spoof.status().is_success());
    let body: Value = spoof.json().await.unwrap();
    if let Some(arr) = body["data"]["todos"].as_array() {
        for r in arr {
            assert_ne!(r["owner_id"], "evil", "Bearer must win over spoof headers");
        }
    }

    // Admin token must authenticate (GraphQL 200), even if role claims default to user.
    let admin_gql = client
        .post(format!("{base}/graphql"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {admin_tok}"))
        .json(&json!({"query":"{ todos { todo_id } }"}))
        .send()
        .await
        .expect("admin");
    assert!(
        admin_gql.status().is_success(),
        "admin bearer must auth GraphQL"
    );

    eprintln!("oidc_pg isolation ok todo={tid}");
}

#[test]
fn compose_and_bootstrap_scripts_present() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    assert!(
        root.join("docker/docker-compose.yml").is_file(),
        "docker-compose.yml"
    );
    assert!(root.join("scripts/up.sh").is_file(), "scripts/up.sh");
    let up = std::fs::read_to_string(root.join("scripts/up.sh")).unwrap();
    assert!(up.contains("OIDC_ISSUER"), "bootstrap exports OIDC_ISSUER");
    assert!(up.contains("DATABASE_URL"), "bootstrap exports DATABASE_URL");
}
