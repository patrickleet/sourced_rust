//! Compatibility façade for e2e-ui application APIs.

#![allow(unused_imports)] // re-exports for call sites/tests

pub use crate::application::{
    DISTRIBUTED_ADMIN_CLIENT_SURFACE, DISTRIBUTED_CLIENT_SURFACE, DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
};
pub use crate::modules::compose::build_service;
pub use crate::modules::graphql::{
    build_graphql_engine, dev_identity, distributed_admin_client_surface, distributed_client_surface,
    distributed_public_client_surface, identity_from_env, oidc_bearer_config,
};

#[cfg(test)]
mod client_surface_tests {
    use super::*;
    use distributed::InMemoryRepository;

    #[test]
    fn pool_free_user_and_admin_exports_compile_real_manifests() {
        distributed_client_surface()
            .manifest()
            .expect("normal application client manifest");
        distributed_admin_client_surface()
            .manifest()
            .expect("elevated application client manifest");
    }

    #[test]
    fn application_todos_keep_portable_owner_row_policy_for_optimistic_list_inserts() {
        use distributed::graphql::ClientRowPolicy;

        let manifest = distributed_client_surface().manifest().unwrap();
        let todos = manifest
            .models
            .iter()
            .find(|model| model.typename == "Todos")
            .expect("Todos model on application surface");
        match &todos.row_policy {
            ClientRowPolicy::Predicate { expression } => {
                let text = serde_json::to_string(expression).expect("serialize row policy");
                assert!(
                    text.contains("x-user-id") && text.contains("owner_id"),
                    "owner claim predicate must be client-portable: {text}"
                );
            }
            other => panic!(
                "Todos must not collapse to server-only row policy (blocks optimistic create list membership); got {other:?}"
            ),
        }

        let blob = manifest
            .models
            .iter()
            .find(|model| model.typename == "BlobGames")
            .expect("BlobGames model on application surface");
        assert!(
            matches!(blob.row_policy, ClientRowPolicy::Predicate { .. }),
            "BlobGames should keep portable owner row policy"
        );
    }

    #[test]
    fn chat_manifest_uses_unit_partition_so_lobby_live_can_stay_active() {
        let manifest = distributed_client_surface().manifest().unwrap();
        let program = manifest
            .projection_programs
            .iter()
            .find(|program| program.name == "project_chat_messages")
            .expect("Chat projection program should be exported");
        assert!(
            program.arms.iter().all(|arm| matches!(
                &arm.partition,
                distributed::graphql::ClientProjectionPartition::Unit
            )),
            "lobby chat uses unit partition so the chat_messages live query can advertise \
             supported index evidence (room isolation stays in the GraphQL where clause). \
             Surface-wide live_resume may still be false when owner-scoped models share the surface."
        );
    }

    #[test]
    fn blob_projection_owner_has_no_async_fact_route() {
        let manifest = distributed_client_surface().manifest().unwrap();
        let owner = manifest
            .projectors
            .iter()
            .find(|projector| projector.name == "project_blob")
            .expect("Blob direct owner should be exported");
        assert!(owner.facts.is_empty());
        assert!(!owner.causal_confirmation);

        let repository = InMemoryRepository::new();
        let service = build_service(
            repository.clone(),
            crate::modules::graphql::ClientSurfaceLocks::default(),
            repository,
        );
        let plan = service.subscription_plan();
        for event in [
            "todo.created",
            "todo.renamed",
            "todo.completed",
            "todo.reopened",
            "todo.archived",
            "todo.force_archived",
            "todo.purged",
            "chat_message.posted",
        ] {
            assert!(
                plan.events.iter().any(|candidate| candidate == event),
                "eventual modeled projection must subscribe to {event}"
            );
        }
        for fact in [
            "blob.started",
            "blob.initialized",
            "blob.level_started",
            "blob.moved",
        ] {
            assert!(
                !plan.events.iter().any(|event| event == fact),
                "direct-only Blob ownership must not register an async route for {fact}"
            );
        }
    }

    #[tokio::test]
    async fn graphiql_does_not_change_the_postgres_runtime_client_manifest() {
        let generated = distributed_client_surface().manifest().unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/distributed")
            .unwrap();
        let repository = distributed::PostgresRepository::new(pool.clone());
        let service = build_service(
            repository.clone(),
            distributed::PostgresLockManager::new(pool),
            repository.clone(),
        );
        let engine =
            crate::modules::graphql::build_graphql_engine_with_graphiql(&repository, &service, dev_identity(), None, true)
                .expect("engine");
        let runtime = engine
            .client_manifest_for_application(
                DISTRIBUTED_CLIENT_SURFACE,
                &["admin", "user"],
                &["user"],
            )
            .unwrap();

        assert_eq!(generated, runtime);

        let make_request = || {
            serde_json::from_value(serde_json::json!({
                "query": "{ todos @skip(if: true) { todo_id } }",
                "extensions": {
                    "distributed": {
                        "client": {
                            "surface": {
                                "kind": "application",
                                "name": DISTRIBUTED_CLIENT_SURFACE,
                                "eligible_roles": ["admin", "user"],
                                "schema_roles": ["user"]
                            },
                            "schemaHash": generated.schema_fingerprint
                        }
                    }
                }
            }))
            .expect("generated application request")
        };
        let mut session = distributed::microsvc::Session::new();
        session.set("x-roles", "user");
        session.set("x-user-id", "person-1");
        let response = engine.execute(&session, make_request()).await;
        assert!(
            !response.is_err(),
            "the runtime must accept the generated application surface: {:?}",
            response.errors
        );
        // Multi-role admin principal may open the same portable contract.
        let mut admin = session.clone();
        admin.set("x-roles", "admin,user");
        let admin_response = engine.execute(&admin, make_request()).await;
        assert!(
            !admin_response.is_err(),
            "admin with user asserted roles must open e2e-ui: {:?}",
            admin_response.errors
        );
        let envelope = response
            .extensions
            .get("distributed")
            .expect("distributed protocol envelope");
        let envelope = serde_json::to_value(envelope).expect("serialized protocol envelope");
        assert_eq!(
            envelope["schemaHash"], generated.schema_fingerprint,
            "the authoritative response must attest the generated schema"
        );
    }

    /// Empty-session open of e2e-ui-public + chat query (anonymous privilege).
    ///
    /// Bare protocol path for unauthenticated lobby peeks; UI route `/public`
    /// documents the same surface name and extension shape.
    #[tokio::test]
    async fn public_surface_opens_and_queries_chat_without_identity() {
        let generated = distributed_public_client_surface().manifest().unwrap();
        assert_eq!(
            generated.surface,
            distributed::graphql::ClientSurfaceIdentity::application(
                DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
                ["anonymous"],
                ["anonymous"],
            )
        );
        let repository = distributed::SqliteRepository::connect_and_migrate("sqlite::memory:")
            .await
            .expect("sqlite memory repo");
        let registry = e2e_readmodels::distributed_manifest()
            .table_registry()
            .expect("registry");
        repository
            .bootstrap_table_schema_for_dev(&registry)
            .await
            .expect("bootstrap tables");
        let service = build_service(
            repository.clone(),
            crate::modules::graphql::ClientSurfaceLocks::default(),
            repository.clone(),
        );
        let engine =
            crate::modules::graphql::build_graphql_engine_with_graphiql(&repository, &service, dev_identity(), None, false)
                .expect("engine");
        let runtime = engine
            .client_manifest_for_application(DISTRIBUTED_PUBLIC_CLIENT_SURFACE, &["anonymous"], &["anonymous"])
            .expect("public surface registered");
        assert_eq!(generated.schema_fingerprint, runtime.schema_fingerprint);

        let request = serde_json::from_value(serde_json::json!({
            "query": "{ chat_messages(limit: 5, offset: 0) { message_id body room_id } }",
            "extensions": {
                "distributed": {
                    "client": {
                        "surface": {
                            "kind": "application",
                            "name": DISTRIBUTED_PUBLIC_CLIENT_SURFACE,
                            "eligible_roles": ["anonymous"],
                            "schema_roles": ["anonymous"]
                        },
                        "schemaHash": generated.schema_fingerprint
                    }
                }
            }
        }))
        .expect("public application request");

        // No x-user-id, no x-roles — unauthenticated principal.
        let session = distributed::microsvc::Session::new();
        let response = engine.execute(&session, request).await;
        assert!(
            !response.is_err(),
            "anonymous open + chat query must succeed: {:?}",
            response.errors
        );
        let data = response.data.into_json().expect("json data");
        assert!(
            data.get("chat_messages").and_then(|v| v.as_array()).is_some(),
            "expected chat_messages array: {data}"
        );
        let envelope = response
            .extensions
            .get("distributed")
            .expect("distributed protocol envelope");
        let envelope = serde_json::to_value(envelope).expect("serialized protocol envelope");
        assert_eq!(envelope["schemaHash"], generated.schema_fingerprint);
    }

    #[test]
    fn module_inventory_lists_todo_chat_blob_identity() {
        assert_eq!(
            crate::E2E_UI_MODULE_IDS,
            &["todo", "chat", "blob", "identity"]
        );
        assert_eq!(crate::application::MODULE_DECLARATIONS.len(), 4);
    }
}