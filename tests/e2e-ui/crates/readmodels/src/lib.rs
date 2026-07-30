//! Read models and projection programs for the e2e-ui fixture.
//!
//! Domain crates own aggregates, replay events, and outward state contracts.
//! This crate consumes those contracts to own query models, projection
//! programs, and deployment-level relationships.

mod blob_projection;
mod chat_projection;
pub mod models;
mod todo_projection;

pub use blob_projection::{BlobDirectEligibilityGuards, BLOB_GAMES};
pub use chat_projection::CHAT_MESSAGES;
pub use models::{
    map_zitadel_user_status, map_zitadel_user_upsert, AuthUsers, BlobGames, ChatMessages, Todos,
    ZitadelEmail, ZitadelUserPayload,
};
pub use todo_projection::{complete_preview, TODO_READS};

pub fn distributed_manifest() -> distributed::DistributedProjectManifest {
    use distributed::RelationalReadModel;

    distributed::DistributedProjectManifest::new("e2e-ui")
        .table_schema(Todos::schema().clone())
        .table_schema(ChatMessages::schema().clone())
        .table_schema(BlobGames::schema().clone())
        .table_schema(AuthUsers::schema().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use blob_domain::{demo_map, BlobGame};
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
    fn referencing_models_own_one_way_auth_user_relationships() {
        let todo = Todos::schema();
        let blob = BlobGames::schema();
        let chat = ChatMessages::schema();
        assert!(blob
            .relationships
            .iter()
            .any(|relationship| relationship.field_name == "owner"
                && relationship.target_model == "AuthUsers"));
        assert!(chat
            .relationships
            .iter()
            .any(|relationship| relationship.field_name == "author"
                && relationship.target_model == "AuthUsers"));

        assert!(todo
            .relationships
            .iter()
            .any(|relationship| relationship.field_name == "owner"
                && relationship.target_model == "AuthUsers"));

        assert!(AuthUsers::schema().relationships.is_empty());

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
        assert!(object("Todos").contains("\n  owner: AuthUsers"));
        assert!(object("BlobGames").contains("\n  owner: AuthUsers"));
        assert!(object("ChatMessages").contains("\n  author: AuthUsers"));
    }
}
