use super::*;

impl<DB> ReadModelWritePlanStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c mut DB::Connection: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'q> &'q str: Encode<'q, DB> + Type<DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn read_model_capabilities(&self) -> ReadModelAdapterCapabilities {
        sql_read_model_capabilities()
    }

    fn commit_write_plan(
        &self,
        plan: ReadModelWritePlan,
    ) -> impl Future<Output = Result<ReadModelCommitOutcome, ReadModelError>> + Send + '_ {
        async move {
            let tables: std::collections::BTreeSet<String> = plan
                .mutations
                .iter()
                .map(|m| m.table_name().to_string())
                .collect();
            validate_sql_write_plan(&plan)?;
            let mut tx = begin_read_model_tx(&self.pool).await?;
            reject_causal_table_writes_in_tx(&mut tx, &tables).await?;
            let outcome = apply_read_model_write_plan_in_tx(&mut tx, plan).await?;
            if self.notify_enabled && !tables.is_empty() {
                DB::push_change_notify(&mut *tx, &tables).await?;
            }
            commit_read_model_tx(tx).await?;
            if outcome.was_applied() && !tables.is_empty() {
                self.publish_read_model_change(crate::ReadModelChange { tables });
            }
            Ok(outcome)
        }
    }
}

impl<DB> RelationalReadModelQueryStore for SqlxRepository<DB>
where
    DB: SqlxRepoBackend,
    for<'c> &'c Pool<DB>: Executor<'c, Database = DB>,
    DB::Arguments: IntoArguments<DB>,
    for<'q> i64: Encode<'q, DB> + Type<DB> + sqlx::Decode<'q, DB>,
    for<'r> &'r str: sqlx::ColumnIndex<DB::Row>,
{
    fn read_model_query_capabilities(&self) -> ReadModelQueryCapabilities {
        ReadModelQueryCapabilities::relationship_includes()
    }

    fn load_graph(
        &self,
        request: ReadModelLoadRequest,
    ) -> impl Future<Output = Result<ReadModelLoadGraph, ReadModelError>> + Send + '_ {
        async move {
            load_read_model_graph(
                &self.pool,
                &self.read_model_schemas,
                request,
                self.read_model_query_capabilities(),
            )
            .await
        }
    }
}
