//! Export GraphQL command catalog for TypeScript command clients.
//!
//! Uses the **same** `e2e_service::graphql_commands()` registry as the running
//! engine — not a hand-maintained parallel list.
//!
//! Offline: no database required (registry is pure).
//!
//! Usage (from `tests/e2e-ui`):
//!   cargo run -p e2e-runner --bin e2e-export-commands -- ui/src/lib/api/commands.manifest.json
//!   make export-commands

use std::env;
use std::path::PathBuf;

use e2e_service::graphql_commands;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let out = env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_out_path);

    let catalog = graphql_commands();
    let body = catalog.catalog_json_pretty()?;
    let header = "\
// GENERATED — source of truth is e2e_service::graphql_commands().\n\
// Regenerate: `make export-commands` (from tests/e2e-ui)\n\
// Then: `make gen-commands` / `npm run gen:commands`\n\
// Spec: distributed GitKB specs/query-layer/references/command-client-dx\n\
// NOTE: This header is stripped if the file is pure JSON; keep as sibling comment via Makefile if needed.\n";

    // Pure JSON for generators (no comment header — JSON.parse).
    let _ = header;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Stable trailing newline for git.
    let body = if body.ends_with('\n') {
        body
    } else {
        format!("{body}\n")
    };
    std::fs::write(&out, &body)?;
    eprintln!(
        "e2e-export-commands: wrote {} ({} bytes, {} commands)",
        out.display(),
        body.len(),
        catalog.catalog().commands.len()
    );
    Ok(())
}

fn default_out_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/src/lib/api/commands.manifest.json")
}
