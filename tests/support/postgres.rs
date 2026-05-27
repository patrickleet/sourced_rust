#![cfg(feature = "postgres")]

use std::env;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use sourced_rust::PostgresRepository;
use sqlx::postgres::PgPoolOptions;
use sqlx::Executor;

static NEXT_SCHEMA_ID: AtomicU64 = AtomicU64::new(1);

pub struct PostgresTestSchema {
    database_url: String,
    schema_name: String,
}

impl PostgresTestSchema {
    pub async fn create_from_env(prefix: &str, skip_message: &str) -> Option<Self> {
        let Ok(database_url) = env::var("DATABASE_URL") else {
            eprintln!("{skip_message}: DATABASE_URL is not set");
            return None;
        };

        Some(
            Self::create(&database_url, prefix)
                .await
                .expect("Postgres test schema should create"),
        )
    }

    pub async fn create(database_url: &str, prefix: &str) -> Result<Self, sqlx::Error> {
        let schema_name = unique_schema_name(prefix);
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url)
            .await?;
        sqlx::query(&format!("CREATE SCHEMA {}", quote_identifier(&schema_name)))
            .execute(&admin_pool)
            .await?;
        admin_pool.close().await;

        Ok(Self {
            database_url: database_url.to_string(),
            schema_name,
        })
    }

    #[allow(dead_code)]
    pub fn schema_name(&self) -> &str {
        &self.schema_name
    }

    pub async fn repository(&self) -> PostgresRepository {
        let repo = PostgresRepository::new(
            PgPoolOptions::new()
                .max_connections(5)
                .after_connect({
                    let schema_name = self.schema_name.clone();
                    move |connection, _metadata| {
                        let search_path =
                            format!("SET search_path TO {}", quote_identifier(&schema_name));
                        Box::pin(async move {
                            connection.execute(search_path.as_str()).await?;
                            Ok(())
                        })
                    }
                })
                .connect(&self.database_url)
                .await
                .expect("Postgres test repository should connect"),
        );
        repo.migrate()
            .await
            .expect("Postgres test repository should migrate");
        repo
    }
}

fn unique_schema_name(prefix: &str) -> String {
    let prefix = sanitize_prefix(prefix);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after epoch")
        .as_nanos();
    let sequence = NEXT_SCHEMA_ID.fetch_add(1, Ordering::Relaxed);
    format!("sr_{prefix}_{nanos}_{sequence}")
}

fn sanitize_prefix(prefix: &str) -> String {
    let mut sanitized = prefix
        .chars()
        .filter_map(|character| {
            if character.is_ascii_alphanumeric() {
                Some(character.to_ascii_lowercase())
            } else if character == '_' {
                Some(character)
            } else {
                None
            }
        })
        .take(18)
        .collect::<String>();

    if sanitized.is_empty() {
        sanitized.push_str("test");
    }

    sanitized
}

fn quote_identifier(identifier: &str) -> String {
    let escaped = identifier.replace('"', "\"\"");
    format!("\"{escaped}\"")
}
