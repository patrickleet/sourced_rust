//! Provider-imported read models for the e2e-ui fixture.
//!
//! Todo, Chat, and Blob own their natural query models beside their reusable
//! domain-event projections. This crate owns only the identity-provider view
//! that relates those domain models without introducing a crate cycle.

pub mod models;

pub use models::{
    map_zitadel_user_status, map_zitadel_user_upsert, AuthUserView, ZitadelEmail,
    ZitadelUserPayload,
};

/// Compose the cross-bounded-context query relationship without making the
/// Chat domain depend on the identity-provider read model crate.
pub fn chat_messages_schema() -> distributed::TableSchema {
    use chat_domain::ChatMessages;
    use distributed::{RelationshipDef, RelationshipKind, RelationalReadModel};

    let mut schema = ChatMessages::schema().clone();
    schema.relationships.push(RelationshipDef {
        field_name: "author".into(),
        kind: RelationshipKind::BelongsTo,
        target_model: "AuthUserView".into(),
        foreign_key: Some("author_id".into()),
        through: None,
        target_foreign_key: None,
    });
    schema
}

/// Compose the Blob owner relationship at the deployment boundary.
pub fn blob_games_schema() -> distributed::TableSchema {
    use blob_domain::BlobGames;
    use distributed::{RelationshipDef, RelationshipKind, RelationalReadModel};

    let mut schema = BlobGames::schema().clone();
    schema.relationships.push(RelationshipDef {
        field_name: "owner".into(),
        kind: RelationshipKind::BelongsTo,
        target_model: "AuthUserView".into(),
        foreign_key: Some("owner_id".into()),
        through: None,
        target_foreign_key: None,
    });
    schema
}

pub fn distributed_manifest() -> distributed::DistributedProjectManifest {
    use distributed::RelationalReadModel;
    use todo_domain::Todos;

    distributed::DistributedProjectManifest::new("e2e-ui")
        .table_schema(Todos::schema().clone())
        .table_schema(chat_messages_schema())
        .table_schema(blob_games_schema())
        .table_schema(AuthUserView::schema().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use blob_domain::{demo_map, BlobGame, BlobGames};
    use chat_domain::ChatMessages;
    use distributed::RelationalReadModel;

    #[test]
    fn canonical_blob_row_reflects_post_move_state() {
        let mut g = BlobGame::default();
        g.start_with_demo("g1", "alice").unwrap();
        let before = BlobGames::from(&g.state());
        assert_eq!(before.score, 0);
        assert!(!before.player_dead);

        g.move_dir("alice", blob_domain::Direction::Right).unwrap();
        let after = BlobGames::from(&g.state());
        assert_eq!(after.score, 1);
        assert_eq!(after.game_id, "g1");
        assert_eq!(after.owner_id, "alice");
        assert!(after.map_json.contains("2")); // visited tile
                                               // Map JSON must differ from pre-move
        assert_ne!(before.map_json, after.map_json);
        // Demo map player moved off origin
        let map: Vec<Vec<u8>> = serde_json::from_str(&after.map_json).unwrap();
        assert_eq!(map[0][0], 2); // visited
        assert_eq!(map[0][1], 9); // player
    }

    #[test]
    fn demo_map_json_roundtrip() {
        let m = demo_map();
        let s = serde_json::to_string(&m).unwrap();
        let back: Vec<Vec<u8>> = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn deployment_relationships_preserve_canonical_projection_storage_identity() {
        let blob = blob_games_schema();
        let chat = chat_messages_schema();
        assert!(blob.has_same_storage_contract(BlobGames::schema()));
        assert!(chat.has_same_storage_contract(ChatMessages::schema()));
        assert!(blob
            .relationships
            .iter()
            .any(|relationship| relationship.field_name == "owner"
                && relationship.target_model == "AuthUserView"));
        assert!(chat
            .relationships
            .iter()
            .any(|relationship| relationship.field_name == "author"
                && relationship.target_model == "AuthUserView"));

        let auth = AuthUserView::schema();
        for field in ["blob_games", "chat_messages"] {
            assert!(auth
                .relationships
                .iter()
                .any(|relationship| relationship.field_name == field));
        }

        let project = distributed_manifest();
        let surface = distributed::graphql::build_surface(
            &project.tables,
            &distributed::graphql::SurfaceOptions::sqlite(),
        )
        .unwrap();
        let sdl = distributed::graphql::graphql_sdl_from_surface(&surface).unwrap();
        let object = |name: &str| {
            let start = sdl.find(&format!("type {name} {{")).unwrap();
            let tail = &sdl[start..];
            &tail[..tail.find("\n}\n").unwrap()]
        };
        assert!(object("BlobGames").contains("\n  owner: AuthUserView"));
        assert!(object("ChatMessages").contains("\n  author: AuthUserView"));
        assert!(object("AuthUserView").contains("\n  blob_games("));
        assert!(object("AuthUserView").contains("\n  chat_messages("));
    }
}
