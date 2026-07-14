//! Export role-scoped GraphQL SDL for SvelteKit codegen.
//!
//! Builds the **same** GraphQL engine as the e2e-ui runner (`build_graphql_engine`),
//! then writes `GraphqlEngine::sdl_for_role` to a file consumed by `npm run gen:gql`.
//!
//! Offline: in-memory SQLite + DevHeaders — no Docker/OIDC required.
//!
//! Usage (from `tests/e2e-ui`):
//!   cargo run -p e2e-runner --bin e2e-export-sdl -- ui/schema/user.graphql
//!   make export-sdl
//!
//! Env:
//!   GQL_ROLE — role to export (default: `user`)

use std::env;
use std::path::PathBuf;

use distributed::SqliteRepository;
use e2e_service::{build_graphql_engine, dev_identity, distributed_manifest};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let role = env::var("GQL_ROLE").unwrap_or_else(|_| "user".into());
    let out = env::args().nth(1).map(PathBuf::from).unwrap_or_else(default_out_path);

    let repo = SqliteRepository::connect_and_migrate("sqlite::memory:").await?;
    let registry = distributed_manifest()
        .table_registry()
        .map_err(|e| format!("manifest: {e}"))?;
    repo.bootstrap_table_schema_for_dev(&registry).await?;

    // Same registration as e2e-ui process (models + command mutations + roles).
    let engine = build_graphql_engine(repo.pool().clone(), dev_identity(), None)?;
    let sdl = engine
        .sdl_for_role(&role)
        .ok_or_else(|| format!("no GraphQL schema registered for role `{role}`"))?;

    let header = format!(
        "# GENERATED — do not treat as hand-authored source of truth.\n\
         # Source: e2e_service::build_graphql_engine + GraphqlEngine::sdl_for_role(\"{role}\")\n\
         # Regenerate: `make export-sdl` or `npm run gen:schema` (from tests/e2e-ui)\n\
         # Then: `cd ui && npm run gen:gql`  (or `make gen-gql` / `npm run gen`)\n\
         #\n\
         # Spec: hops GitKB specs/e2e-ui/rust-role-sdl-codegen\n\n"
    );

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!("{header}{sdl}");
    std::fs::write(&out, &body)?;
    eprintln!(
        "e2e-export-sdl: wrote {} ({} bytes, role={role})",
        out.display(),
        body.len()
    );
    Ok(())
}

fn default_out_path() -> PathBuf {
    // crates/runner → tests/e2e-ui/ui/schema/user.graphql
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/schema/user.graphql")
}
