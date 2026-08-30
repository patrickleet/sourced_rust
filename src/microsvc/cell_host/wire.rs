//! Strict, size-bounded wire values shared by cell workers and command hosts.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::{OutboxMessage, OutboxMessageStatus};

pub const MAX_CELL_OUTBOX_ITEMS: usize = 256;
pub const MAX_CELL_OUTBOX_PAYLOAD_BYTES: usize = 1024 * 1024;
pub const MAX_CELL_OUTBOX_WIRE_BYTES: usize = 1536 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 512;
const MAX_CODEC_BYTES: usize = 128;
const MAX_METADATA_ENTRIES: usize = 64;
const MAX_METADATA_VALUE_BYTES: usize = 1024;
const CLAIM_WIRE_RESERVE_PER_ITEM: usize = 768;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CellOutboxWireItem {
    pub id: String,
    pub event_type: String,
    pub payload: Vec<u8>,
    pub payload_codec: String,
    pub payload_codec_version: u16,
    pub status: String,
    #[serde(default)]
    pub attempts: u32,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub worker_id: Option<String>,
    #[serde(default)]
    pub leased_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    pub source_aggregate_type: Option<String>,
    pub source_aggregate_id: Option<String>,
    pub source_sequence: Option<u64>,
}

impl CellOutboxWireItem {
    pub fn from_message(message: &OutboxMessage) -> Self {
        Self {
            id: message.id.clone(),
            event_type: message.event_type.clone(),
            payload: message.payload.clone(),
            payload_codec: message.payload_codec.clone(),
            payload_codec_version: message.payload_codec_version,
            status: message.status.as_str().to_string(),
            attempts: message.attempts,
            last_error: message.last_error.clone(),
            worker_id: message.worker_id.clone(),
            leased_until_unix_ms: message.leased_until.and_then(|time| {
                time.duration_since(SystemTime::UNIX_EPOCH)
                    .ok()
                    .and_then(|duration| u64::try_from(duration.as_millis()).ok())
            }),
            metadata: message.metadata.clone(),
            source_aggregate_type: message.source_aggregate_type.clone(),
            source_aggregate_id: message.source_aggregate_id.clone(),
            source_sequence: message.source_sequence,
        }
    }

    pub fn try_into_message(self) -> Result<OutboxMessage, String> {
        self.try_into_message_with_status(&[OutboxMessageStatus::Pending])
    }

    pub fn try_into_stored_message(self) -> Result<OutboxMessage, String> {
        self.try_into_message_with_status(&[
            OutboxMessageStatus::Pending,
            OutboxMessageStatus::InFlight,
            OutboxMessageStatus::Published,
            OutboxMessageStatus::Failed,
        ])
    }

    fn try_into_message_with_status(
        self,
        allowed_statuses: &[OutboxMessageStatus],
    ) -> Result<OutboxMessage, String> {
        validate_identifier("outbox id", &self.id)?;
        validate_identifier("event type", &self.event_type)?;
        if self.payload.len() > MAX_CELL_OUTBOX_PAYLOAD_BYTES {
            return Err("cell outbox payload exceeds 1 MiB".into());
        }
        if self.payload_codec.is_empty()
            || self.payload_codec.len() > MAX_CODEC_BYTES
            || self.payload_codec_version == 0
        {
            return Err("cell outbox payload codec is invalid".into());
        }
        let status = self
            .status
            .parse::<OutboxMessageStatus>()
            .map_err(|_| "cell outbox status is invalid")?;
        if !allowed_statuses.contains(&status) {
            return Err("cell outbox row has an invalid delivery status for this response".into());
        }
        match status {
            OutboxMessageStatus::Pending
                if self.worker_id.is_some() || self.leased_until_unix_ms.is_some() =>
            {
                return Err("pending cell outbox row must not carry lease ownership".into())
            }
            OutboxMessageStatus::InFlight
                if self.worker_id.is_none() || self.leased_until_unix_ms.is_none() =>
            {
                return Err("claimed cell outbox row is missing lease ownership".into())
            }
            _ => {}
        }
        validate_metadata(&self.metadata)?;
        if let Some(worker_id) = &self.worker_id {
            validate_identifier("outbox worker id", worker_id)?;
        }
        if self
            .last_error
            .as_ref()
            .is_some_and(|error| error.len() > MAX_METADATA_VALUE_BYTES)
        {
            return Err("cell outbox last error is too large".into());
        }
        if let Some(value) = &self.source_aggregate_type {
            validate_identifier("source aggregate type", value)?;
        }
        if let Some(value) = &self.source_aggregate_id {
            validate_identifier("source aggregate id", value)?;
        }
        let source_fields = [
            self.source_aggregate_type.is_some(),
            self.source_aggregate_id.is_some(),
            self.source_sequence.is_some(),
        ];
        if source_fields.iter().any(|present| *present)
            && source_fields.iter().any(|present| !*present)
        {
            return Err("cell outbox source identity must be complete or absent".into());
        }
        if self.source_sequence == Some(0) {
            return Err("cell outbox source sequence must be nonzero".into());
        }
        let mut message = OutboxMessage::create_with_metadata(
            self.id,
            self.event_type,
            self.payload,
            self.metadata,
        )
        .map_err(|error| error.to_string())?;
        message.payload_codec = self.payload_codec;
        message.payload_codec_version = self.payload_codec_version;
        message.status = status;
        message.attempts = self.attempts;
        message.last_error = self.last_error;
        message.worker_id = self.worker_id;
        message.leased_until = self
            .leased_until_unix_ms
            .map(|millis| {
                SystemTime::UNIX_EPOCH
                    .checked_add(Duration::from_millis(millis))
                    .ok_or_else(|| "cell outbox lease timestamp is out of range".to_string())
            })
            .transpose()?;
        message.source_aggregate_type = self.source_aggregate_type;
        message.source_aggregate_id = self.source_aggregate_id;
        message.source_sequence = self.source_sequence;
        Ok(message)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CellWaitPathRequest {
    pub command_id: String,
    #[serde(default)]
    pub input: serde_json::Value,
}

impl CellWaitPathRequest {
    pub fn parse(value: serde_json::Value) -> Result<Self, String> {
        let request: Self = serde_json::from_value(value)
            .map_err(|error| format!("invalid cell wait-path envelope: {error}"))?;
        if request.command_id.trim().is_empty()
            || request.command_id.len() > MAX_IDENTIFIER_BYTES
            || request.command_id.chars().any(char::is_control)
        {
            return Err("cell wait-path commandId is invalid".into());
        }
        Ok(request)
    }
}

pub fn parse_cell_outbox(value: &serde_json::Value) -> Result<Vec<OutboxMessage>, String> {
    parse_cell_outbox_with(value, CellOutboxWireItem::try_into_message)
}

/// Reject a cell command's outbox before commit if its bounded HTTP form could
/// not later be claimed and published by the host.
pub(crate) fn validate_cell_outbox_messages(messages: &[OutboxMessage]) -> Result<(), String> {
    let value = serde_json::json!({
        "outbox": messages
            .iter()
            .map(CellOutboxWireItem::from_message)
            .collect::<Vec<_>>()
    });
    parse_cell_outbox(&value)?;
    let encoded = serde_json::to_vec(&value)
        .map_err(|error| format!("cell outbox could not be encoded: {error}"))?;
    let claimed_size_upper_bound = encoded
        .len()
        .saturating_add(messages.len().saturating_mul(CLAIM_WIRE_RESERVE_PER_ITEM));
    if claimed_size_upper_bound > MAX_CELL_OUTBOX_WIRE_BYTES {
        return Err("cell outbox wire envelope exceeds 1.5 MiB".into());
    }
    Ok(())
}

fn parse_cell_outbox_with(
    value: &serde_json::Value,
    convert: fn(CellOutboxWireItem) -> Result<OutboxMessage, String>,
) -> Result<Vec<OutboxMessage>, String> {
    let Some(items) = value.get("outbox") else {
        return Ok(Vec::new());
    };
    let items: Vec<CellOutboxWireItem> =
        serde_json::from_value(items.clone()).map_err(|error| format!("cell outbox: {error}"))?;
    if items.len() > MAX_CELL_OUTBOX_ITEMS {
        return Err(format!(
            "cell outbox contains more than {MAX_CELL_OUTBOX_ITEMS} rows"
        ));
    }
    let mut total_payload = 0_usize;
    let mut ids = std::collections::HashSet::new();
    items
        .into_iter()
        .map(|item| {
            if !ids.insert(item.id.clone()) {
                return Err("cell outbox response contains duplicate ids".into());
            }
            total_payload = total_payload.saturating_add(item.payload.len());
            if total_payload > MAX_CELL_OUTBOX_PAYLOAD_BYTES {
                return Err("cell outbox payloads exceed 1 MiB in total".into());
            }
            convert(item)
        })
        .collect()
}

fn validate_identifier(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES || value.chars().any(char::is_control)
    {
        return Err(format!("{label} is invalid"));
    }
    Ok(())
}

fn validate_metadata(metadata: &HashMap<String, String>) -> Result<(), String> {
    if metadata.len() > MAX_METADATA_ENTRIES {
        return Err("cell outbox metadata has too many entries".into());
    }
    for (key, value) in metadata {
        if key.is_empty()
            || key.len() > MAX_METADATA_VALUE_BYTES
            || value.len() > MAX_METADATA_VALUE_BYTES
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
        {
            return Err("cell outbox metadata is invalid".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_item() -> serde_json::Value {
        json!({
            "id": "event-1",
            "eventType": "todo.created",
            "payload": [0, 127, 255],
            "payloadCodec": "bytes",
            "payloadCodecVersion": 1,
            "status": "pending",
            "metadata": {},
            "sourceAggregateType": "todo",
            "sourceAggregateId": "todo-1",
            "sourceSequence": 1
        })
    }

    #[test]
    fn parses_strict_pending_item_without_numeric_truncation() {
        let rows = parse_cell_outbox(&json!({ "outbox": [valid_item()] })).expect("valid");
        assert_eq!(rows[0].payload, vec![0, 127, 255]);

        let mut invalid = valid_item();
        invalid["payload"] = json!([256]);
        assert!(parse_cell_outbox(&json!({ "outbox": [invalid] })).is_err());
    }

    #[test]
    fn rejects_unknown_fields_and_status() {
        let mut unknown = valid_item();
        unknown["forged"] = json!(true);
        assert!(parse_cell_outbox(&json!({ "outbox": [unknown] })).is_err());

        let mut published = valid_item();
        published["status"] = json!("published");
        assert!(parse_cell_outbox(&json!({ "outbox": [published] })).is_err());

        assert!(CellWaitPathRequest::parse(json!({
            "commandId": "command-1",
            "input": {},
            "roles": ["admin"]
        }))
        .is_err());
    }

    #[test]
    fn enforces_row_and_total_payload_limits() {
        let items = (0..=MAX_CELL_OUTBOX_ITEMS)
            .map(|index| {
                let mut item = valid_item();
                item["id"] = json!(format!("event-{index}"));
                item
            })
            .collect::<Vec<_>>();
        assert!(parse_cell_outbox(&json!({ "outbox": items })).is_err());

        let mut oversized = valid_item();
        oversized["payload"] = json!(vec![0_u8; MAX_CELL_OUTBOX_PAYLOAD_BYTES + 1]);
        assert!(parse_cell_outbox(&json!({ "outbox": [oversized] })).is_err());
    }

    #[test]
    fn precommit_validation_enforces_the_encoded_wire_budget() {
        let rows = (0..MAX_CELL_OUTBOX_ITEMS)
            .map(|index| {
                OutboxMessage::create_with_metadata(
                    format!("event-{index}"),
                    "todo.created",
                    vec![255; 4_096],
                    HashMap::new(),
                )
                .expect("message")
            })
            .collect::<Vec<_>>();
        assert!(validate_cell_outbox_messages(&rows).is_err());
    }
}
