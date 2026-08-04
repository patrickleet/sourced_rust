//! Client WASM surface for blob pure reduces.
//!
//! JS calls [`blob_simulate_move`] with `map_json`, score, and direction; gets
//! JSON fields for the optimistic patch (or null / undefined on fail-closed).

use blob_core::{simulate_move, Direction};
use wasm_bindgen::prelude::*;

/// Apply one move for known-row optimism.
///
/// Returns a JSON object:
/// `{ map_json, score, player_dead, current_level_completed, status }`
/// or `undefined` when the move is impossible / input is invalid.
#[wasm_bindgen(js_name = blobSimulateMove)]
pub fn blob_simulate_move(map_json: &str, score: f64, direction: &str) -> Option<String> {
    if !score.is_finite() {
        return None;
    }
    // JS numbers are f64; board scores are small integers.
    let score = score as i64;
    let direction = Direction::parse(direction)?;
    let map: Vec<Vec<u8>> = serde_json::from_str(map_json).ok()?;
    let preview = simulate_move(&map, score, direction).ok()?;
    let map_json = serde_json::to_string(&preview.map).ok()?;
    let body = serde_json::json!({
        "map_json": map_json,
        "score": preview.score,
        "player_dead": preview.player_dead,
        "current_level_completed": preview.level_complete,
        "status": preview.status(),
    });
    Some(body.to_string())
}
