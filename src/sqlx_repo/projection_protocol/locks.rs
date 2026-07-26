use super::*;

pub(crate) const PROJECTION_CHANGE_NOTIFY_TABLE: &str = "projection_changes";

/// Reject ordinary/raw table writes once a model-wide causal owner exists.
///
/// Both this path and `register_projection_models` first acquire the same
/// durable per-table ownership fence in sorted order. The marker check and all
/// physical writes remain inside that transaction, so an absent marker cannot
/// race a first legacy row into a newly causal-owned table.
pub(crate) async fn reject_causal_table_writes_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    tables: &BTreeSet<String>,
) -> Result<(), TableStoreError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    lock_projection_table_ownership_fences_in_tx(tx, tables).await?;
    for table in tables {
        let mut builder = QueryBuilder::<DB>::new(
            "SELECT table_name FROM projection_causal_tables WHERE table_name = ",
        );
        builder.push_bind(table.as_str());
        builder.push(" LIMIT 1");
        let row = builder
            .build()
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| {
                crate::sqlx_repo::read_model_storage_error(
                    DB::BACKEND,
                    "check causal projection ownership",
                    error,
                )
            })?;
        if row.is_some() {
            return Err(TableStoreError::CausalWriteRequired {
                table: table.clone(),
            });
        }
    }
    Ok(())
}

pub(super) async fn lock_projection_table_ownership_fences_in_tx<DB>(
    tx: &mut Transaction<'_, DB>,
    tables: &BTreeSet<String>,
) -> Result<(), TableStoreError>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
{
    // BTreeSet iteration is deterministic, preventing multi-table writers and
    // registration batches from deadlocking on opposite lock orders.
    for table in tables {
        let mut insert = QueryBuilder::<DB>::new(
            "INSERT INTO projection_table_ownership_fences (table_name) VALUES (",
        );
        insert.push_bind(table.as_str());
        insert.push(") ON CONFLICT (table_name) DO NOTHING");
        insert.build().execute(&mut **tx).await.map_err(|error| {
            crate::sqlx_repo::read_model_storage_error(
                DB::BACKEND,
                "acquire causal projection ownership fence",
                error,
            )
        })?;

        let mut lock = QueryBuilder::<DB>::new(
            "SELECT table_name FROM projection_table_ownership_fences WHERE table_name = ",
        );
        lock.push_bind(table.as_str());
        if DB::BACKEND == "postgres" {
            lock.push(" FOR UPDATE");
        }
        if lock
            .build()
            .fetch_optional(&mut **tx)
            .await
            .map_err(|error| {
                crate::sqlx_repo::read_model_storage_error(
                    DB::BACKEND,
                    "lock causal projection ownership fence",
                    error,
                )
            })?
            .is_none()
        {
            return Err(TableStoreError::Storage(format!(
                "{} causal projection ownership fence `{table}` disappeared",
                DB::BACKEND
            )));
        }
    }
    Ok(())
}
