//! Read models for the e2e-ui fixture (todos + chat + blob games + auth users).
//! Projected only from domain / provider events (never from commands).

pub mod models;

pub use models::{
    map_blob_fact, map_chat_posted, map_fact, map_todo_fact, map_zitadel_user_status,
    map_zitadel_user_upsert, AuthUserView, BlobGameView, ChatMessageView, TodoView, ZitadelEmail,
    ZitadelUserPayload,
};

pub fn distributed_manifest() -> distributed::DistributedProjectManifest {
    use distributed::RelationalReadModel;
    distributed::DistributedProjectManifest::new("e2e-ui")
        .table_schema(TodoView::schema().clone())
        .table_schema(ChatMessageView::schema().clone())
        .table_schema(BlobGameView::schema().clone())
        .table_schema(AuthUserView::schema().clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use blob_domain::{demo_map, BlobGame, BlobGameFact};

    #[test]
    fn projector_map_reflects_post_move_fact() {
        let mut g = BlobGame::default();
        g.start_with_demo("g1", "alice").unwrap();
        let before = map_blob_fact(&BlobGameFact::from_game(&g));
        assert_eq!(before.score, 0);
        assert!(!before.player_dead);

        g.move_dir("alice", blob_domain::Direction::Right).unwrap();
        let after = map_blob_fact(&BlobGameFact::from_game(&g));
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
}
