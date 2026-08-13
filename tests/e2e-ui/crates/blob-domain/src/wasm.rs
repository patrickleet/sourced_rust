//! Client WASM surface for pure board rules (`--features wasm`).
//!
//! One export: JSON record + JSON args → JSON assign fields (or undefined).
//! All validation lives here so the TS host stays a thin framework bridge.

use crate::core::{simulate_move, Direction};
use serde_json::{Map, Value};
use wasm_bindgen::prelude::*;

/// Known-row pure reduce for `blob.simulate_move`.
///
/// * `record_json` — known `BlobGames` row (needs `map_json`, `score`)
/// * `args_json` — command args (needs `direction`)
///
/// Returns assign payload:
/// `{ map_json, score, player_dead, current_level_completed, status }`
/// or `undefined` when invalid / impossible (fail closed).
#[wasm_bindgen(js_name = blobSimulateMove)]
pub fn blob_simulate_move(record_json: &str, args_json: &str) -> Option<String> {
    let record: Value = serde_json::from_str(record_json).ok()?;
    let args: Value = serde_json::from_str(args_json).ok()?;
    let map_json = record.get("map_json")?.as_str()?;
    let score = json_i64(record.get("score")?)?;
    let direction = args.get("direction")?.as_str().and_then(Direction::parse)?;
    let map: Vec<Vec<u8>> = serde_json::from_str(map_json).ok()?;
    let preview = simulate_move(&map, score, direction).ok()?;
    let next_map_json = serde_json::to_string(&preview.map).ok()?;

    let mut out = Map::new();
    out.insert("map_json".into(), Value::String(next_map_json));
    out.insert("score".into(), Value::from(preview.score));
    out.insert("player_dead".into(), Value::Bool(preview.player_dead));
    out.insert(
        "current_level_completed".into(),
        Value::Bool(preview.level_complete),
    );
    out.insert("status".into(), Value::String(preview.status()));
    serde_json::to_string(&Value::Object(out)).ok()
}

fn json_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().and_then(|u| i64::try_from(u).ok()))
            .or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}
