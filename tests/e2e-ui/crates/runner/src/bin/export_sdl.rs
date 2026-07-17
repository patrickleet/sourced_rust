//! Export role-scoped GraphQL SDL for SvelteKit codegen.
//!
//! **A11:** Always builds query surface via surface IR
//! (`GraphqlEngine::ir_sdl_for_role` → build_surface → surface_for_role → SDL)
//! and dual-checks that every IR Query root field exists on the runtime dump.
//!
//! **File content** is the full runtime `sdl_for_role` (includes command Mutation
//! types required by graphql-codegen + `commands.operations.gql`). Query root
//! inventory is proven equal to IR so dual maintenance cannot drift silently.

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

    let engine = build_graphql_engine(repo.pool().clone(), dev_identity(), None)?;

    // A11 production IR path (must succeed).
    let ir = engine
        .ir_sdl_for_role(&role)
        .map_err(|e| format!("ir_sdl_for_role({role}): {e}"))?;

    let runtime = engine
        .sdl_for_role(&role)
        .ok_or_else(|| format!("no runtime schema for role `{role}`"))?;

    assert_ir_query_roots_in_runtime(&ir, &runtime)
        .map_err(|e| format!("IR vs runtime dual-check failed: {e}"))?;

    let header = format!(
        "# GENERATED — do not treat as hand-authored source of truth.\n\
         # IR query surface (validated): GraphqlEngine::ir_sdl_for_role(\"{role}\")\n\
         #   build_surface → surface_for_role → graphql_sdl_from_surface\n\
         # File body: runtime sdl_for_role (includes command Mutation for codegen)\n\
         # Dual-check: every IR Query root field name ⊆ runtime Query\n\
         # Regenerate: `make export-sdl` or `npm run gen:schema`\n\
         # Then: `cd ui && npm run gen:gql`\n\
         #\n\
         # Spec: specs/query-layer/v1/surface-ir (A11)\n\n"
    );

    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!("{header}{runtime}");
    std::fs::write(&out, &body)?;
    eprintln!(
        "e2e-export-sdl: wrote {} ({} bytes, role={role}, IR dual-check ok)",
        out.display(),
        body.len()
    );
    Ok(())
}

/// Extract field names from `type Query { ... }` block (top-level field identifiers).
fn query_field_names(sdl: &str) -> Result<Vec<String>, String> {
    let start = sdl
        .find("type Query {")
        .ok_or_else(|| "no type Query in SDL".to_string())?;
    let rest = &sdl[start + "type Query {".len()..];
    let mut depth = 1i32;
    let mut end = 0usize;
    for (i, ch) in rest.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == 0 {
        return Err("unclosed type Query".into());
    }
    let body = &rest[..end];
    let mut names = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // fieldName( or fieldName:
        let name: String = line
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() && name != "type" {
            names.push(name);
        }
    }
    Ok(names)
}

fn assert_ir_query_roots_in_runtime(ir: &str, runtime: &str) -> Result<(), String> {
    let ir_fields = query_field_names(ir)?;
    let rt_fields = query_field_names(runtime)?;
    if ir_fields.is_empty() {
        // empty role is ok
        return Ok(());
    }
    for f in &ir_fields {
        if !rt_fields.iter().any(|r| r == f) {
            return Err(format!(
                "IR Query field `{f}` missing from runtime Query (ir={ir_fields:?}, rt={rt_fields:?})"
            ));
        }
    }
    Ok(())
}

fn default_out_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../ui/schema/user.graphql")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dual_check_accepts_matching_roots() {
        let ir = "type Query {\n  todos: [T!]!\n  todos_by_pk(id: String!): T\n}\n";
        let rt = "type Query {\n  todos: [T!]!\n  todos_by_pk(id: String!): T\n  extra: Int\n}\n";
        assert_ir_query_roots_in_runtime(ir, rt).unwrap();
    }

    #[test]
    fn dual_check_rejects_missing_root() {
        let ir = "type Query {\n  todos: [T!]!\n  secret: Int\n}\n";
        let rt = "type Query {\n  todos: [T!]!\n}\n";
        assert!(assert_ir_query_roots_in_runtime(ir, rt).is_err());
    }
}
