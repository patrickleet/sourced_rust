use super::*;

pub(crate) fn event_from_row<DB>(row: DB::Row) -> Result<EventRecord, RepositoryError>
where
    DB: SqlxRepoBackend,
    for<'q> i64: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> String: Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> Vec<u8>: Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    let payload_codec: String = row
        .try_get("payload_codec")
        .map_err(|err| repository_storage_error::<DB>("decode payload codec row", err))?;
    // Nearly every row carries the crate's own codec constant; borrow it
    // instead of keeping a per-event allocation.
    let payload_codec = if payload_codec == BITCODE_PAYLOAD_CODEC {
        Cow::Borrowed(BITCODE_PAYLOAD_CODEC)
    } else {
        Cow::Owned(payload_codec)
    };
    let payload_codec_version = repository_u16_from_i64(
        DB::BACKEND,
        row.try_get("payload_codec_version").map_err(|err| {
            repository_storage_error::<DB>("decode payload codec version row", err)
        })?,
        "payload_codec_version",
    )?;
    let metadata_json: String = row
        .try_get("metadata")
        .map_err(|err| repository_storage_error::<DB>("decode metadata row", err))?;
    let metadata = deserialize_event_metadata(&metadata_json)?;
    let event = EventRecord {
        event_name: row
            .try_get("event_name")
            .map_err(|err| repository_storage_error::<DB>("decode event name row", err))?,
        payload_codec,
        payload_codec_version,
        payload: row
            .try_get("payload")
            .map_err(|err| repository_storage_error::<DB>("decode payload row", err))?,
        event_version: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("event_version")
                .map_err(|err| repository_storage_error::<DB>("decode event version row", err))?,
            "event_version",
        )?,
        sequence: repository_u64_from_i64(
            DB::BACKEND,
            row.try_get("sequence")
                .map_err(|err| repository_storage_error::<DB>("decode sequence row", err))?,
            "sequence",
        )?,
        timestamp: DB::decode_timestamp(&row, "recorded_at")?,
        metadata,
    };
    validate_supported_event_codec(&event)?;
    Ok(event)
}
