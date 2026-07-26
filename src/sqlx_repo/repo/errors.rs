use super::*;

/// Convert a [`SystemTime`] to epoch seconds for database-side comparisons.
pub(crate) fn system_time_epoch_secs<DB: SqlxRepoBackend>(
    timestamp: SystemTime,
) -> Result<f64, RepositoryError> {
    let duration = timestamp.duration_since(UNIX_EPOCH).map_err(|err| {
        RepositoryError::Model(format!(
            "timestamp before UNIX epoch cannot be stored in {}: {err}",
            DB::BACKEND
        ))
    })?;
    Ok(duration.as_secs_f64())
}

pub(crate) fn repository_storage_error<DB: SqlxRepoBackend>(
    operation: &str,
    err: sqlx::Error,
) -> RepositoryError {
    crate::sqlx_repo::repository_storage_error(DB::BACKEND, operation, err)
}
