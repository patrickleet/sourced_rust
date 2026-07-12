//! Selection set → single SQL statement per root field (dialect-portable JSON tree).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_graphql::Value;
use serde_json::Value as JsonValue;

use crate::microsvc::Session;
use crate::table::{
    resolve_m2m_target_foreign_key, ColumnType, RelationshipKind, TableSchema,
};

use super::engine::{CatalogEntry, EngineInner, RoleModelPerm};
use super::naming::is_valid_graphql_name;
use super::filter::{CmpOp, FilterExpr, LitValue, Operand};
use super::permissions::SelectPermission;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlDialect {
    Postgres,
    Sqlite,
}

#[derive(Clone, Debug)]
pub struct SqlPlan {
    pub sql: String,
    pub binds: Vec<BindValue>,
    /// JSON paths (dot-separated response keys) that need hex→base64 rewrite (SQLite Bytes).
    pub bytes_hex_paths: Vec<String>,
    pub tables_touched: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum BindValue {
    Null,
    Bool(bool),
    I64(i64),
    F64(f64),
    Text(String),
    Bytes(Vec<u8>),
    Json(JsonValue),
}

#[derive(Clone, Debug)]
pub struct SelectionNode {
    pub response_key: String,
    pub field_name: String,
    pub args: BTreeMap<String, Value>,
    pub children: Vec<SelectionNode>,
}

/// Compile a root field selection into one SQL statement.
pub fn compile_root(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    model_name: &str,
    kind: RootKind,
    selection: &SelectionNode,
) -> Result<SqlPlan, String> {
    let entry = inner
        .catalog
        .get(model_name)
        .ok_or_else(|| format!("unknown model `{model_name}`"))?;
    let perm = inner
        .permissions
        .get(&(model_name.to_string(), role.to_string()))
        .map(|p| &p.permission)
        .ok_or_else(|| format!("role `{role}` has no permission on `{model_name}`"))?;

    let mut binds = Vec::new();
    let mut bytes_paths = Vec::new();
    let mut tables = vec![entry.schema.table_name.clone()];
    let alias = "t0";

    let limit = resolve_limit(
        selection.args.get("limit"),
        perm.limit,
        inner.default_limit,
        inner.max_limit,
    );
    let offset = selection
        .args
        .get("offset")
        .and_then(value_as_u64)
        .unwrap_or(0);

    let where_sql = compile_where(
        inner,
        session,
        role,
        &entry.schema,
        perm,
        selection.args.get("where"),
        alias,
        &mut binds,
        &mut tables,
        0,
    )?;

    let order_sql = compile_order_by(
        &entry.schema,
        selection.args.get("order_by"),
        alias,
        perm,
    )?;

    let projection = compile_object_projection(
        inner,
        session,
        role,
        &entry.schema,
        perm,
        selection,
        alias,
        &mut binds,
        &mut bytes_paths,
        &mut tables,
        "",
        0,
    )?;

    let (json_agg, coalesce_empty, json_cast) = match inner.dialect {
        SqlDialect::Postgres => ("jsonb_agg", "'[]'::jsonb", ""),
        // json() ensures json_object TEXT is treated as JSON, not a JSON string.
        SqlDialect::Sqlite => ("json_group_array", "'[]'", "json"),
    };

    let sql = match kind {
        RootKind::List => {
            let agg_arg = if json_cast.is_empty() {
                "root".to_string()
            } else {
                format!("{json_cast}(root)")
            };
            format!(
                "SELECT coalesce({json_agg}({agg_arg}), {coalesce_empty}) FROM (\n  SELECT {projection} AS root\n  FROM \"{}\" {alias}\n  WHERE {where_sql}\n  {order_sql}\n  LIMIT {} OFFSET {}\n) sub",
                entry.schema.table_name,
                {
                    binds.push(BindValue::I64(limit as i64));
                    placeholder(inner.dialect, binds.len())
                },
                {
                    binds.push(BindValue::I64(offset as i64));
                    placeholder(inner.dialect, binds.len())
                }
            )
        }
        RootKind::ByPk => {
            // PK equality already in where via args as field args.
            let mut pk_preds = Vec::new();
            for pk in &entry.schema.primary_key.columns {
                let v = selection.args.get(pk).ok_or_else(|| {
                    format!("missing primary key argument `{pk}`")
                })?;
                let col = entry
                    .schema
                    .columns
                    .iter()
                    .find(|c| c.column_name == *pk)
                    .ok_or_else(|| format!("pk column `{pk}` missing"))?;
                let bind = value_to_bind(v, &col.column_type)?;
                binds.push(bind);
                let ph = placeholder(inner.dialect, binds.len());
                pk_preds.push(format!("{alias}.\"{pk}\" = {ph}"));
            }
            let pk_where = pk_preds.join(" AND ");
            let full_where = if where_sql == "TRUE" || where_sql == "true" {
                pk_where
            } else {
                format!("({pk_where}) AND ({where_sql})")
            };
            format!(
                "SELECT {projection} FROM \"{}\" {alias} WHERE {full_where} LIMIT 1",
                entry.schema.table_name
            )
        }
        RootKind::Aggregate => {
            // Simplified aggregate: count + nodes.
            let nodes_proj = compile_object_projection(
                inner,
                session,
                role,
                &entry.schema,
                perm,
                selection
                    .children
                    .iter()
                    .find(|c| c.field_name == "nodes")
                    .unwrap_or(selection),
                alias,
                &mut binds,
                &mut bytes_paths,
                &mut tables,
                "nodes",
                0,
            )?;
            format!(
                "SELECT {build_obj}('aggregate', {build_obj}('count', (SELECT count(*) FROM \"{table}\" {alias} WHERE {where_sql})), 'nodes', coalesce((SELECT {json_agg}(n) FROM (SELECT {nodes_proj} AS n FROM \"{table}\" {alias} WHERE {where_sql} {order_sql} LIMIT {lim} OFFSET {off}) x), {coalesce_empty}))",
                build_obj = match inner.dialect {
                    SqlDialect::Postgres => "jsonb_build_object",
                    SqlDialect::Sqlite => "json_object",
                },
                table = entry.schema.table_name,
                lim = {
                    binds.push(BindValue::I64(limit as i64));
                    placeholder(inner.dialect, binds.len())
                },
                off = {
                    binds.push(BindValue::I64(offset as i64));
                    placeholder(inner.dialect, binds.len())
                },
            )
        }
    };

    Ok(SqlPlan {
        sql,
        binds,
        bytes_hex_paths: bytes_paths,
        tables_touched: tables,
    })
}

#[derive(Clone, Copy)]
pub enum RootKind {
    List,
    ByPk,
    Aggregate,
}

fn validate_response_key(key: &str) -> Result<(), String> {
    if is_valid_graphql_name(key) {
        Ok(())
    } else {
        Err(format!("invalid GraphQL response key `{key}`"))
    }
}

fn placeholder(dialect: SqlDialect, n: usize) -> String {
    match dialect {
        SqlDialect::Postgres => format!("${n}"),
        SqlDialect::Sqlite => "?".into(),
    }
}

fn resolve_limit(
    client: Option<&Value>,
    role_limit: Option<u64>,
    default_limit: u64,
    max_limit: u64,
) -> u64 {
    let client = client.and_then(value_as_u64).unwrap_or(default_limit);
    let with_role = role_limit.map(|r| client.min(r)).unwrap_or(client);
    with_role.min(max_limit)
}

fn value_as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64().or_else(|| n.as_i64().map(|i| i as u64)),
        _ => None,
    }
}

fn compile_object_projection(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    schema: &TableSchema,
    perm: &SelectPermission,
    selection: &SelectionNode,
    alias: &str,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    tables: &mut Vec<String>,
    path_prefix: &str,
    depth: usize,
) -> Result<String, String> {
    if depth > inner.max_depth {
        return Err("max depth exceeded".into());
    }
    let mut pairs: Vec<(String, String)> = Vec::new();

    // If no children, project all allowed columns.
    let fields: Vec<&SelectionNode> = if selection.children.is_empty() {
        Vec::new()
    } else {
        selection.children.iter().collect()
    };

    if fields.is_empty() {
        for col in schema.columns.iter().filter(|c| !c.skipped) {
            if !perm.allows_column(&col.column_name) {
                continue;
            }
            let expr = column_json_expr(inner.dialect, alias, col, binds)?;
            if matches!(col.column_type, ColumnType::Bytes)
                && matches!(inner.dialect, SqlDialect::Sqlite)
            {
                let p = if path_prefix.is_empty() {
                    col.column_name.clone()
                } else {
                    format!("{path_prefix}.{}", col.column_name)
                };
                bytes_paths.push(p);
            }
            validate_response_key(&col.column_name)?;
            pairs.push((col.column_name.clone(), expr));
        }
    } else {
        for child in fields {
            if child.field_name.ends_with("_aggregate") {
                // Nested aggregate: simplified skip or count-only.
                continue;
            }
            if let Some(col) = schema
                .columns
                .iter()
                .find(|c| c.column_name == child.field_name && !c.skipped)
            {
                if !perm.allows_column(&col.column_name) {
                    continue;
                }
                let expr = column_json_expr(inner.dialect, alias, col, binds)?;
                if matches!(col.column_type, ColumnType::Bytes)
                    && matches!(inner.dialect, SqlDialect::Sqlite)
                {
                    let p = if path_prefix.is_empty() {
                        child.response_key.clone()
                    } else {
                        format!("{path_prefix}.{}", child.response_key)
                    };
                    bytes_paths.push(p);
                }
                validate_response_key(&child.response_key)?;
                pairs.push((child.response_key.clone(), expr));
                continue;
            }
            if let Some(rel) = schema
                .relationships
                .iter()
                .find(|r| r.field_name == child.field_name)
            {
                let target_entry = match inner.catalog.get(&rel.target_model) {
                    Some(e) => e,
                    None => continue,
                };
                let target_perm = match inner
                    .permissions
                    .get(&(rel.target_model.clone(), role.to_string()))
                {
                    Some(p) => &p.permission,
                    None => continue, // untracked for role
                };
                tables.push(target_entry.schema.table_name.clone());
                let child_path = if path_prefix.is_empty() {
                    child.response_key.clone()
                } else {
                    format!("{path_prefix}.{}", child.response_key)
                };
                let sub = compile_relationship_subquery(
                    inner,
                    session,
                    role,
                    schema,
                    alias,
                    rel,
                    target_entry,
                    target_perm,
                    child,
                    binds,
                    bytes_paths,
                    tables,
                    &child_path,
                    depth + 1,
                )?;
                validate_response_key(&child.response_key)?;
                pairs.push((child.response_key.clone(), sub));
            }
        }
    }

    Ok(chunked_json_object(inner.dialect, &pairs))
}

fn column_json_expr(
    dialect: SqlDialect,
    alias: &str,
    col: &crate::table::TableColumn,
    _binds: &mut Vec<BindValue>,
) -> Result<String, String> {
    let q = format!("{alias}.\"{}\"", col.column_name);
    Ok(match (&col.column_type, dialect) {
        (ColumnType::Timestamp, SqlDialect::Postgres) => format!("{q}::text"),
        (ColumnType::Bytes, SqlDialect::Postgres) => format!("encode({q}, 'base64')"),
        (ColumnType::Bytes, SqlDialect::Sqlite) => format!("hex({q})"),
        (ColumnType::Json, SqlDialect::Postgres) => format!("{q}"),
        _ => q,
    })
}

fn chunked_json_object(dialect: SqlDialect, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return match dialect {
            SqlDialect::Postgres => "jsonb_build_object()".into(),
            SqlDialect::Sqlite => "json_object()".into(),
        };
    }
    let build = match dialect {
        SqlDialect::Postgres => "jsonb_build_object",
        SqlDialect::Sqlite => "json_object",
    };
    let chunks: Vec<&[(String, String)]> = pairs.chunks(40).collect();
    if chunks.len() == 1 {
        return format!(
            "{build}({})",
            pairs
                .iter()
                .map(|(k, v)| format!("'{k}', {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    match dialect {
        SqlDialect::Postgres => {
            let parts: Vec<String> = chunks
                .iter()
                .map(|chunk| {
                    format!(
                        "{build}({})",
                        chunk
                            .iter()
                            .map(|(k, v)| format!("'{k}', {v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })
                .collect();
            parts.join(" || ")
        }
        SqlDialect::Sqlite => {
            // Nested json_insert
            let mut expr = format!(
                "{build}({})",
                chunks[0]
                    .iter()
                    .map(|(k, v)| format!("'{k}', {v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            for chunk in chunks.iter().skip(1) {
                let inserts = chunk
                    .iter()
                    .map(|(k, v)| format!("'$.{k}', {v}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                expr = format!("json_insert({expr}, {inserts})");
            }
            expr
        }
    }
}

fn compile_relationship_subquery(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    source: &TableSchema,
    source_alias: &str,
    rel: &crate::table::RelationshipDef,
    target: &CatalogEntry,
    target_perm: &SelectPermission,
    selection: &SelectionNode,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    tables: &mut Vec<String>,
    path_prefix: &str,
    depth: usize,
) -> Result<String, String> {
    let child_alias = format!("t{depth}");
    let limit = resolve_limit(
        selection.args.get("limit"),
        target_perm.limit,
        inner.default_limit,
        inner.max_limit,
    );
    let offset = selection
        .args
        .get("offset")
        .and_then(value_as_u64)
        .unwrap_or(0);

    let fk = rel.foreign_key.as_deref().unwrap_or("");
    let fk_col = column_name_for(source, fk).unwrap_or(fk);
    let target_fk_col = column_name_for(&target.schema, fk).unwrap_or(fk);

    let join_pred = match rel.kind {
        RelationshipKind::HasMany => {
            format!("{child_alias}.\"{target_fk_col}\" = {source_alias}.\"{fk_col}\"")
        }
        RelationshipKind::BelongsTo => {
            format!("{child_alias}.\"{target_fk_col}\" = {source_alias}.\"{fk_col}\"")
        }
        RelationshipKind::ManyToMany => {
            let through_name = rel
                .through
                .as_deref()
                .ok_or_else(|| "m2m missing through".to_string())?;
            let through_model = inner
                .by_table
                .get(through_name)
                .and_then(|m| inner.catalog.get(m))
                .ok_or_else(|| format!("through table `{through_name}` not in catalog"))?;
            let target_fk = resolve_m2m_target_foreign_key(
                source,
                rel,
                &through_model.schema,
                &target.schema,
            )
            .map_err(|e| e.to_string())?;
            let source_join_col = column_name_for(&through_model.schema, fk).unwrap_or(fk);
            // Source PK for join (single-column assumption with FK fallback).
            let source_pk = source
                .primary_key
                .columns
                .first()
                .map(|s| s.as_str())
                .unwrap_or("id");
            let target_pk = target
                .schema
                .primary_key
                .columns
                .first()
                .map(|s| s.as_str())
                .unwrap_or("id");
            tables.push(through_name.to_string());
            return compile_m2m_subquery(
                inner,
                session,
                role,
                source_alias,
                source_pk,
                through_name,
                source_join_col,
                &target_fk,
                &target.schema,
                target_pk,
                target_perm,
                selection,
                binds,
                bytes_paths,
                tables,
                path_prefix,
                depth,
                limit,
                offset,
            );
        }
    };

    let where_extra = compile_where(
        inner,
        session,
        role,
        &target.schema,
        target_perm,
        selection.args.get("where"),
        &child_alias,
        binds,
        tables,
        depth,
    )?;
    let order_sql = compile_order_by(
        &target.schema,
        selection.args.get("order_by"),
        &child_alias,
        target_perm,
    )?;
    let projection = compile_object_projection(
        inner,
        session,
        role,
        &target.schema,
        target_perm,
        selection,
        &child_alias,
        binds,
        bytes_paths,
        tables,
        path_prefix,
        depth,
    )?;

    let (json_agg, coalesce_empty) = match inner.dialect {
        SqlDialect::Postgres => ("jsonb_agg", "'[]'::jsonb"),
        SqlDialect::Sqlite => ("json_group_array", "'[]'"),
    };

    match rel.kind {
        RelationshipKind::BelongsTo => Ok(format!(
            "(SELECT {projection} FROM \"{}\" {child_alias} WHERE {join_pred} AND ({where_extra}) LIMIT 1)",
            target.schema.table_name
        )),
        _ => {
            binds.push(BindValue::I64(limit as i64));
            let lim = placeholder(inner.dialect, binds.len());
            binds.push(BindValue::I64(offset as i64));
            let off = placeholder(inner.dialect, binds.len());
            Ok(format!(
                "(SELECT coalesce({json_agg}(obj), {coalesce_empty}) FROM (\n  SELECT {projection} AS obj\n  FROM \"{}\" {child_alias}\n  WHERE {join_pred} AND ({where_extra})\n  {order_sql}\n  LIMIT {lim} OFFSET {off}\n) inner_rows)",
                target.schema.table_name
            ))
        }
    }
}

fn compile_m2m_subquery(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    source_alias: &str,
    source_pk: &str,
    through_name: &str,
    source_join_col: &str,
    target_fk: &str,
    target_schema: &TableSchema,
    target_pk: &str,
    target_perm: &SelectPermission,
    selection: &SelectionNode,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    tables: &mut Vec<String>,
    path_prefix: &str,
    depth: usize,
    limit: u64,
    offset: u64,
) -> Result<String, String> {
    let child_alias = format!("t{depth}");
    let j_alias = format!("j{depth}");
    let where_extra = compile_where(
        inner,
        session,
        role,
        target_schema,
        target_perm,
        selection.args.get("where"),
        &child_alias,
        binds,
        tables,
        depth,
    )?;
    let order_sql = compile_order_by(
        target_schema,
        selection.args.get("order_by"),
        &child_alias,
        target_perm,
    )?;
    let projection = compile_object_projection(
        inner,
        session,
        role,
        target_schema,
        target_perm,
        selection,
        &child_alias,
        binds,
        bytes_paths,
        tables,
        path_prefix,
        depth,
    )?;
    let (json_agg, coalesce_empty) = match inner.dialect {
        SqlDialect::Postgres => ("jsonb_agg", "'[]'::jsonb"),
        SqlDialect::Sqlite => ("json_group_array", "'[]'"),
    };
    binds.push(BindValue::I64(limit as i64));
    let lim = placeholder(inner.dialect, binds.len());
    binds.push(BindValue::I64(offset as i64));
    let off = placeholder(inner.dialect, binds.len());
    Ok(format!(
        "(SELECT coalesce({json_agg}(obj), {coalesce_empty}) FROM (\n  SELECT {projection} AS obj\n  FROM \"{target_table}\" {child_alias}\n  JOIN \"{through_name}\" {j_alias} ON {j_alias}.\"{target_fk}\" = {child_alias}.\"{target_pk}\"\n  WHERE {j_alias}.\"{source_join_col}\" = {source_alias}.\"{source_pk}\"\n    AND ({where_extra})\n  {order_sql}\n  LIMIT {lim} OFFSET {off}\n) x)",
        target_table = target_schema.table_name,
    ))
}

fn column_name_for<'a>(schema: &'a TableSchema, name: &str) -> Option<&'a str> {
    schema.columns.iter().find_map(|c| {
        if c.column_name == name || c.field_name == name {
            Some(c.column_name.as_str())
        } else {
            None
        }
    })
}

fn compile_order_by(
    schema: &TableSchema,
    order_arg: Option<&Value>,
    alias: &str,
    perm: &SelectPermission,
) -> Result<String, String> {
    let mut parts = Vec::new();
    if let Some(Value::List(items)) = order_arg {
        for item in items {
            if let Value::Object(map) = item {
                for (col, dir) in map {
                    if !perm.allows_column(col) {
                        continue;
                    }
                    if !schema.columns.iter().any(|c| c.column_name == *col) {
                        continue;
                    }
                    let dir_s = match dir {
                        Value::Enum(e) => e.as_str(),
                        Value::String(s) => s.as_str(),
                        _ => "asc",
                    };
                    let sql_dir = match dir_s {
                        "desc" | "desc_nulls_first" | "desc_nulls_last" => "DESC",
                        _ => "ASC",
                    };
                    let nulls = match dir_s {
                        "asc_nulls_first" | "desc_nulls_first" => " NULLS FIRST",
                        "asc_nulls_last" | "desc_nulls_last" => " NULLS LAST",
                        _ => "",
                    };
                    parts.push(format!("{alias}.\"{col}\" {sql_dir}{nulls}"));
                }
            }
        }
    }
    // Always append PK asc tiebreaker.
    for pk in &schema.primary_key.columns {
        parts.push(format!("{alias}.\"{pk}\" ASC"));
    }
    if parts.is_empty() {
        Ok(String::new())
    } else {
        Ok(format!("ORDER BY {}", parts.join(", ")))
    }
}

fn compile_where(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    schema: &TableSchema,
    perm: &SelectPermission,
    client_where: Option<&Value>,
    alias: &str,
    binds: &mut Vec<BindValue>,
    tables: &mut Vec<String>,
    depth: usize,
) -> Result<String, String> {
    if depth > inner.max_depth {
        return Err("max depth exceeded".into());
    }
    let mut preds = Vec::new();
    if let Some(filter) = &perm.filter {
        preds.push(compile_filter_expr(
            inner, session, schema, filter, alias, binds, tables, depth,
        )?);
    }
    if let Some(w) = client_where {
        preds.push(compile_client_where(
            inner, session, role, schema, perm, w, alias, binds, tables, depth,
        )?);
    }
    if preds.is_empty() {
        Ok("TRUE".into())
    } else {
        Ok(preds.join(" AND "))
    }
}

fn compile_filter_expr(
    inner: &EngineInner,
    session: &Session,
    schema: &TableSchema,
    expr: &FilterExpr,
    alias: &str,
    binds: &mut Vec<BindValue>,
    tables: &mut Vec<String>,
    depth: usize,
) -> Result<String, String> {
    match expr {
        FilterExpr::And(xs) => {
            if xs.is_empty() {
                return Ok("TRUE".into());
            }
            let parts: Result<Vec<_>, _> = xs
                .iter()
                .map(|x| {
                    compile_filter_expr(inner, session, schema, x, alias, binds, tables, depth + 1)
                })
                .collect();
            Ok(format!("({})", parts?.join(" AND ")))
        }
        FilterExpr::Or(xs) => {
            if xs.is_empty() {
                return Ok("FALSE".into());
            }
            let parts: Result<Vec<_>, _> = xs
                .iter()
                .map(|x| {
                    compile_filter_expr(inner, session, schema, x, alias, binds, tables, depth + 1)
                })
                .collect();
            Ok(format!("({})", parts?.join(" OR ")))
        }
        FilterExpr::Not(x) => Ok(format!(
            "NOT ({})",
            compile_filter_expr(inner, session, schema, x, alias, binds, tables, depth + 1)?
        )),
        FilterExpr::Cmp { column, op, rhs } => {
            let col = schema
                .columns
                .iter()
                .find(|c| c.column_name == *column)
                .ok_or_else(|| format!("unknown column `{column}`"))?;
            let bind = operand_to_bind(rhs, session, &col.column_type)?;
            binds.push(bind);
            let ph = placeholder(inner.dialect, binds.len());
            let col_ref = format!("{alias}.\"{column}\"");
            let sql_op = match op {
                CmpOp::Eq => "=",
                CmpOp::Neq => "<>",
                CmpOp::Gt => ">",
                CmpOp::Gte => ">=",
                CmpOp::Lt => "<",
                CmpOp::Lte => "<=",
                CmpOp::Like => "LIKE",
                CmpOp::Ilike => match inner.dialect {
                    SqlDialect::Postgres => "ILIKE",
                    SqlDialect::Sqlite => "LIKE",
                },
                CmpOp::Contains => "@>",
                CmpOp::ContainedIn => "<@",
                CmpOp::HasKey => "?",
            };
            let cast_ph = cast_placeholder(inner.dialect, &col.column_type, &ph);
            Ok(format!("{col_ref} {sql_op} {cast_ph}"))
        }
        FilterExpr::In {
            column,
            values,
            negated,
        } => {
            if values.is_empty() {
                return Ok(if *negated {
                    "TRUE".into()
                } else {
                    "FALSE".into()
                });
            }
            if values.len() > inner.max_in_list {
                return Err(format!(
                    "_in list length {} exceeds max_in_list {}",
                    values.len(),
                    inner.max_in_list
                ));
            }
            let col = schema
                .columns
                .iter()
                .find(|c| c.column_name == *column)
                .ok_or_else(|| format!("unknown column `{column}`"))?;
            let col_ref = format!("{alias}.\"{column}\"");
            match inner.dialect {
                SqlDialect::Postgres => {
                    // Expand as IN for simplicity (array bind is dialect-specific).
                    let mut phs = Vec::new();
                    for v in values {
                        let bind = operand_to_bind(v, session, &col.column_type)?;
                        binds.push(bind);
                        phs.push(placeholder(inner.dialect, binds.len()));
                    }
                    let op = if *negated { "NOT IN" } else { "IN" };
                    Ok(format!("{col_ref} {op} ({})", phs.join(", ")))
                }
                SqlDialect::Sqlite => {
                    let mut phs = Vec::new();
                    for v in values {
                        let bind = operand_to_bind(v, session, &col.column_type)?;
                        binds.push(bind);
                        phs.push(placeholder(inner.dialect, binds.len()));
                    }
                    let op = if *negated { "NOT IN" } else { "IN" };
                    Ok(format!("{col_ref} {op} ({})", phs.join(", ")))
                }
            }
        }
        FilterExpr::IsNull { column, is_null } => {
            let col_ref = format!("{alias}.\"{column}\"");
            Ok(if *is_null {
                format!("{col_ref} IS NULL")
            } else {
                format!("{col_ref} IS NOT NULL")
            })
        }
        FilterExpr::Rel { field, predicate } => {
            let rel = schema
                .relationships
                .iter()
                .find(|r| r.field_name == *field)
                .ok_or_else(|| format!("unknown relationship `{field}`"))?;
            let target = inner
                .catalog
                .get(&rel.target_model)
                .ok_or_else(|| format!("rel target `{}` not in catalog", rel.target_model))?;
            tables.push(target.schema.table_name.clone());
            let child_alias = format!("r{depth}");
            let fk = rel.foreign_key.as_deref().unwrap_or("");
            let inner_pred = compile_filter_expr(
                inner,
                session,
                &target.schema,
                predicate,
                &child_alias,
                binds,
                tables,
                depth + 1,
            )?;
            match rel.kind {
                RelationshipKind::HasMany => {
                    let target_fk = column_name_for(&target.schema, fk).unwrap_or(fk);
                    let source_col = column_name_for(schema, fk).unwrap_or(
                        schema
                            .primary_key
                            .columns
                            .first()
                            .map(|s| s.as_str())
                            .unwrap_or("id"),
                    );
                    Ok(format!(
                        "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {child_alias}.\"{target_fk}\" = {alias}.\"{source_col}\" AND ({inner_pred}))",
                        target.schema.table_name
                    ))
                }
                RelationshipKind::BelongsTo => {
                    let source_fk = column_name_for(schema, fk).unwrap_or(fk);
                    let target_pk = column_name_for(&target.schema, fk).unwrap_or(
                        target
                            .schema
                            .primary_key
                            .columns
                            .first()
                            .map(|s| s.as_str())
                            .unwrap_or("id"),
                    );
                    Ok(format!(
                        "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {child_alias}.\"{target_pk}\" = {alias}.\"{source_fk}\" AND ({inner_pred}))",
                        target.schema.table_name
                    ))
                }
                RelationshipKind::ManyToMany => {
                    let through = rel
                        .through
                        .as_deref()
                        .ok_or_else(|| "m2m rel missing through".to_string())?;
                    let through_entry = inner
                        .by_table
                        .get(through)
                        .and_then(|m| inner.catalog.get(m))
                        .ok_or_else(|| format!("through `{through}` missing"))?;
                    let target_fk = resolve_m2m_target_foreign_key(
                        schema,
                        rel,
                        &through_entry.schema,
                        &target.schema,
                    )
                    .map_err(|e| e.to_string())?;
                    let source_pk = schema
                        .primary_key
                        .columns
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("id");
                    let target_pk = target
                        .schema
                        .primary_key
                        .columns
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("id");
                    let j = format!("j{depth}");
                    Ok(format!(
                        "EXISTS (SELECT 1 FROM \"{through}\" {j} JOIN \"{}\" {child_alias} ON {j}.\"{target_fk}\" = {child_alias}.\"{target_pk}\" WHERE {j}.\"{fk}\" = {alias}.\"{source_pk}\" AND ({inner_pred}))",
                        target.schema.table_name
                    ))
                }
            }
        }
    }
}

fn compile_client_where(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    schema: &TableSchema,
    perm: &SelectPermission,
    value: &Value,
    alias: &str,
    binds: &mut Vec<BindValue>,
    tables: &mut Vec<String>,
    depth: usize,
) -> Result<String, String> {
    if depth > inner.max_depth {
        return Err("max depth exceeded".into());
    }
    let Value::Object(map) = value else {
        return Ok("TRUE".into());
    };
    let mut preds = Vec::new();
    for (key, val) in map {
        match key.as_str() {
            "_and" => {
                if let Value::List(items) = val {
                    for item in items {
                        preds.push(compile_client_where(
                            inner, session, role, schema, perm, item, alias, binds, tables, depth + 1,
                        )?);
                    }
                }
            }
            "_or" => {
                if let Value::List(items) = val {
                    let mut parts = Vec::new();
                    for item in items {
                        parts.push(compile_client_where(
                            inner, session, role, schema, perm, item, alias, binds, tables, depth + 1,
                        )?);
                    }
                    if !parts.is_empty() {
                        preds.push(format!("({})", parts.join(" OR ")));
                    }
                }
            }
            "_not" => {
                preds.push(format!(
                    "NOT ({})",
                    compile_client_where(
                        inner, session, role, schema, perm, val, alias, binds, tables, depth + 1,
                    )?
                ));
            }
            col_name => {
                if let Some(col) = schema.columns.iter().find(|c| c.column_name == *col_name) {
                    if !perm.allows_column(col_name) {
                        continue;
                    }
                    if let Value::Object(ops) = val {
                        for (op, rhs) in ops {
                            preds.push(compile_client_op(
                                inner,
                                session,
                                col_name,
                                &col.column_type,
                                op,
                                rhs,
                                alias,
                                binds,
                            )?);
                        }
                    }
                } else if let Some(rel) = schema
                    .relationships
                    .iter()
                    .find(|r| r.field_name == *col_name)
                {
                    // Relationship predicate → EXISTS
                    let target = match inner.catalog.get(&rel.target_model) {
                        Some(t) => t,
                        None => continue,
                    };
                    let target_perm = match inner
                        .permissions
                        .get(&(rel.target_model.clone(), role.to_string()))
                    {
                        Some(p) => &p.permission,
                        None => continue,
                    };
                    let child_alias = format!("cw{depth}");
                    let inner_pred = compile_client_where(
                        inner,
                        session,
                        role,
                        &target.schema,
                        target_perm,
                        val,
                        &child_alias,
                        binds,
                        tables,
                        depth + 1,
                    )?;
                    let fk = rel.foreign_key.as_deref().unwrap_or("");
                    match rel.kind {
                        RelationshipKind::HasMany => {
                            let target_fk = column_name_for(&target.schema, fk).unwrap_or(fk);
                            let source_col = schema
                                .primary_key
                                .columns
                                .first()
                                .map(|s| s.as_str())
                                .unwrap_or("id");
                            preds.push(format!(
                                "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {child_alias}.\"{target_fk}\" = {alias}.\"{source_col}\" AND ({inner_pred}))",
                                target.schema.table_name
                            ));
                        }
                        RelationshipKind::BelongsTo => {
                            let source_fk = column_name_for(schema, fk).unwrap_or(fk);
                            let target_pk = target
                                .schema
                                .primary_key
                                .columns
                                .first()
                                .map(|s| s.as_str())
                                .unwrap_or("id");
                            preds.push(format!(
                                "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {child_alias}.\"{target_pk}\" = {alias}.\"{source_fk}\" AND ({inner_pred}))",
                                target.schema.table_name
                            ));
                        }
                        RelationshipKind::ManyToMany => {
                            // Simplified: skip if through missing
                        }
                    }
                }
            }
        }
    }
    if preds.is_empty() {
        Ok("TRUE".into())
    } else {
        Ok(format!("({})", preds.join(" AND ")))
    }
}

fn compile_client_op(
    inner: &EngineInner,
    _session: &Session,
    column: &str,
    column_type: &ColumnType,
    op: &str,
    rhs: &Value,
    alias: &str,
    binds: &mut Vec<BindValue>,
) -> Result<String, String> {
    let col_ref = format!("{alias}.\"{column}\"");
    match op {
        "_is_null" => {
            let yes = matches!(rhs, Value::Boolean(true));
            Ok(if yes {
                format!("{col_ref} IS NULL")
            } else {
                format!("{col_ref} IS NOT NULL")
            })
        }
        "_in" | "_nin" => {
            let Value::List(items) = rhs else {
                return Err(format!("{op} requires a list"));
            };
            if items.is_empty() {
                return Ok(if op == "_in" {
                    "FALSE".into()
                } else {
                    "TRUE".into()
                });
            }
            if items.len() > inner.max_in_list {
                return Err(format!(
                    "{op} list length {} exceeds max_in_list {}",
                    items.len(),
                    inner.max_in_list
                ));
            }
            let mut phs = Vec::new();
            for item in items {
                binds.push(value_to_bind(item, column_type)?);
                phs.push(placeholder(inner.dialect, binds.len()));
            }
            let sql_op = if op == "_in" { "IN" } else { "NOT IN" };
            Ok(format!("{col_ref} {sql_op} ({})", phs.join(", ")))
        }
        other => {
            let sql_op = match other {
                "_eq" => "=",
                "_neq" => "<>",
                "_gt" => ">",
                "_gte" => ">=",
                "_lt" => "<",
                "_lte" => "<=",
                "_like" => "LIKE",
                "_ilike" => match inner.dialect {
                    SqlDialect::Postgres => "ILIKE",
                    SqlDialect::Sqlite => "LIKE",
                },
                "_contains" => "@>",
                "_contained_in" => "<@",
                "_has_key" => "?",
                _ => return Err(format!("unknown comparison op `{other}`")),
            };
            binds.push(value_to_bind(rhs, column_type)?);
            let ph = placeholder(inner.dialect, binds.len());
            let cast_ph = cast_placeholder(inner.dialect, column_type, &ph);
            Ok(format!("{col_ref} {sql_op} {cast_ph}"))
        }
    }
}

fn cast_placeholder(dialect: SqlDialect, column_type: &ColumnType, ph: &str) -> String {
    match (dialect, column_type) {
        (SqlDialect::Postgres, ColumnType::Timestamp) => format!("{ph}::timestamptz"),
        (SqlDialect::Postgres, ColumnType::Json) => format!("{ph}::jsonb"),
        _ => ph.to_string(),
    }
}

fn operand_to_bind(
    op: &Operand,
    session: &Session,
    column_type: &ColumnType,
) -> Result<BindValue, String> {
    match op {
        Operand::Lit(lit) => lit_to_bind(lit),
        Operand::Claim(c) => {
            let raw = session
                .get(&c.header)
                .or_else(|| session.get(&c.header.to_ascii_lowercase()))
                .ok_or_else(|| format!("missing claim `{}`", c.header))?;
            parse_claim(raw, column_type)
        }
    }
}

fn lit_to_bind(lit: &LitValue) -> Result<BindValue, String> {
    Ok(match lit {
        LitValue::String(s) => BindValue::Text(s.clone()),
        LitValue::I64(i) => BindValue::I64(*i),
        LitValue::F64(f) => BindValue::F64(*f),
        LitValue::Bool(b) => BindValue::Bool(*b),
        LitValue::Json(j) => BindValue::Json(j.clone()),
        LitValue::Null => BindValue::Null,
    })
}

fn parse_claim(raw: &str, column_type: &ColumnType) -> Result<BindValue, String> {
    match column_type {
        ColumnType::Integer | ColumnType::UnsignedInteger => raw
            .parse::<i64>()
            .map(BindValue::I64)
            .map_err(|_| format!("claim value `{raw}` is not an integer")),
        ColumnType::Float => raw
            .parse::<f64>()
            .map(BindValue::F64)
            .map_err(|_| format!("claim value `{raw}` is not a float")),
        ColumnType::Boolean => match raw {
            "true" | "TRUE" | "1" => Ok(BindValue::Bool(true)),
            "false" | "FALSE" | "0" => Ok(BindValue::Bool(false)),
            _ => Err(format!("claim value `{raw}` is not a boolean")),
        },
        ColumnType::Json => Err("claims cannot compare to Json columns".into()),
        _ => Ok(BindValue::Text(raw.to_string())),
    }
}

fn value_to_bind(v: &Value, column_type: &ColumnType) -> Result<BindValue, String> {
    match v {
        Value::Null => Ok(BindValue::Null),
        Value::Boolean(b) => Ok(BindValue::Bool(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(BindValue::I64(i))
            } else if let Some(f) = n.as_f64() {
                Ok(BindValue::F64(f))
            } else {
                Err("number out of range".into())
            }
        }
        Value::String(s) => match column_type {
            ColumnType::Bytes => {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD
                    .decode(s.as_bytes())
                    .map(BindValue::Bytes)
                    .map_err(|e| format!("invalid base64: {e}"))
            }
            ColumnType::Integer | ColumnType::UnsignedInteger => s
                .parse::<i64>()
                .map(BindValue::I64)
                .map_err(|_| format!("expected integer, got `{s}`")),
            _ => Ok(BindValue::Text(s.clone())),
        },
        Value::List(_) | Value::Object(_) | Value::Enum(_) => {
            let json = value_to_json(v)?;
            Ok(BindValue::Json(json))
        }
        _ => Err("unsupported GraphQL value for bind".into()),
    }
}

fn value_to_json(v: &Value) -> Result<JsonValue, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

/// Walk async-graphql selection field into our SelectionNode tree.
pub fn selection_from_field(field: async_graphql::SelectionField<'_>) -> SelectionNode {
    let mut args = BTreeMap::new();
    if let Ok(arg_list) = field.arguments() {
        for (name, value) in arg_list {
            args.insert(name.to_string(), value);
        }
    }
    let mut children = Vec::new();
    for sel in field.selection_set() {
        children.push(selection_from_field(sel));
    }
    SelectionNode {
        response_key: field.alias().unwrap_or_else(|| field.name()).to_string(),
        field_name: field.name().to_string(),
        args,
        children,
    }
}

/// Helper for pure unit tests without an engine.
pub fn compile_list_sql_for_test(
    dialect: SqlDialect,
    schema: &TableSchema,
    where_sql: &str,
    limit: u64,
) -> String {
    let (json_agg, coalesce_empty, build) = match dialect {
        SqlDialect::Postgres => ("jsonb_agg", "'[]'::jsonb", "jsonb_build_object"),
        SqlDialect::Sqlite => ("json_group_array", "'[]'", "json_object"),
    };
    let pairs: Vec<String> = schema
        .columns
        .iter()
        .filter(|c| !c.skipped)
        .map(|c| format!("'{}', t0.\"{}\"", c.column_name, c.column_name))
        .collect();
    format!(
        "SELECT coalesce({json_agg}(root), {coalesce_empty}) FROM (\n  SELECT {build}({}) AS root\n  FROM \"{}\" t0\n  WHERE {where_sql}\n  ORDER BY {}\n  LIMIT {limit} OFFSET 0\n) sub",
        pairs.join(", "),
        schema.table_name,
        schema
            .primary_key
            .columns
            .iter()
            .map(|c| format!("t0.\"{c}\" ASC"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

#[cfg(test)]
mod security_tests {
    use super::*;
    use crate::graphql::naming::is_valid_graphql_name;

    #[test]
    fn response_key_validator_accepts_graphql_names() {
        assert!(validate_response_key("order_id").is_ok());
        assert!(validate_response_key("_x").is_ok());
        assert!(validate_response_key("a1").is_ok());
    }

    #[test]
    fn response_key_validator_rejects_injection_shaped_keys() {
        assert!(validate_response_key("a', (SELECT 1), '").is_err());
        assert!(validate_response_key("a b").is_err());
        assert!(validate_response_key("").is_err());
        assert!(validate_response_key("__proto__").is_err());
        assert!(!is_valid_graphql_name("1bad"));
    }

    #[test]
    fn resolve_limit_clamps_to_max() {
        assert_eq!(resolve_limit(None, None, 100, 1000), 100);
        assert_eq!(
            resolve_limit(Some(&Value::from(9_000_000u64)), None, 100, 1000),
            1000
        );
        assert_eq!(
            resolve_limit(Some(&Value::from(50u64)), Some(10), 100, 1000),
            10
        );
    }
}
