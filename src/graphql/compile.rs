//! Selection set → single SQL statement per root field (dialect-portable JSON tree).
//!
//! # v1 join / PK assumptions
//!
//! Relationship SQL assumes **single-column primary keys** and a single
//! `foreign_key` column per relationship:
//! - **HasMany**: FK lives on the child → `child.fk = parent.pk`
//! - **BelongsTo**: FK lives on the parent → `child.pk = parent.fk`
//! - **ManyToMany**: through-table holds both FKs; join helpers emit
//!   through→target ON + through→parent WHERE fragments
//!
//! Multi-column PKs/FKs are out of scope until a dedicated policy task lands
//! (see maintain-3 / maintain-5). Join equality is centralized in
//! [`join_predicate_direct`] / [`join_predicate_m2m_parent`] /
//! [`join_predicate_m2m_target`]. Dialect SQL fragments live on [`DialectOps`].
#![allow(clippy::only_used_in_recursion, clippy::too_many_arguments)]

use std::collections::BTreeMap;

use async_graphql::Value;
use serde::Serialize;
use serde_json::Value as JsonValue;

use crate::microsvc::Session;
use crate::table::{resolve_m2m_target_foreign_key, ColumnType, RelationshipKind, TableSchema};

use super::engine::{CatalogEntry, EngineInner};
use super::filter::{CmpOp, FilterExpr, LitValue, Operand};
use super::naming::{is_valid_graphql_name, scalar_type_name};
use super::permissions::ReadPermission;

const QUERY_EVIDENCE_HIDDEN_PREFIX: &str = "0__distributed_evidence_pk_";
const MAX_QUERY_EVIDENCE_NODES: usize = 1_024;
const MAX_QUERY_EVIDENCE_KEY_FIELDS: usize = 64;
const MAX_QUERY_EVIDENCE_RECORDS: usize = 4_096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SqlDialect {
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    Postgres,
    Sqlite,
}

/// Dialect-specific SQL fragment table (dedup-4).
///
/// Prefer `dialect.ops()` over ad-hoc match arms for JSON aggregate / object /
/// empty-array / ILIKE strings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialectOps {
    pub json_agg: &'static str,
    pub empty_array: &'static str,
    pub build_object: &'static str,
    /// SQLite wraps list roots with `json(...)`; Postgres leaves this empty.
    pub json_cast_fn: Option<&'static str>,
    /// Case-insensitive LIKE operator (`ILIKE` on PG; `LIKE` on SQLite).
    pub ilike_op: &'static str,
}

impl SqlDialect {
    pub fn ops(self) -> DialectOps {
        match self {
            SqlDialect::Postgres => DialectOps {
                json_agg: "jsonb_agg",
                empty_array: "'[]'::jsonb",
                build_object: "jsonb_build_object",
                json_cast_fn: None,
                ilike_op: "ILIKE",
            },
            SqlDialect::Sqlite => DialectOps {
                json_agg: "json_group_array",
                empty_array: "'[]'",
                build_object: "json_object",
                // Ensures json_object TEXT is treated as JSON, not a JSON string.
                json_cast_fn: Some("json"),
                ilike_op: "LIKE",
            },
        }
    }
}

/// Direct (non-m2m) join equality for HasMany / BelongsTo (dedup-2).
///
/// # Arguments
/// - `fk_col`: resolved SQL column name of the foreign key
///   (on child for HasMany, on parent for BelongsTo)
pub(crate) fn join_predicate_direct(
    kind: RelationshipKind,
    parent_alias: &str,
    child_alias: &str,
    parent_pk: &str,
    child_pk: &str,
    fk_col: &str,
) -> Result<String, String> {
    match kind {
        RelationshipKind::HasMany => Ok(format!(
            "{child_alias}.\"{fk_col}\" = {parent_alias}.\"{parent_pk}\""
        )),
        RelationshipKind::BelongsTo => Ok(format!(
            "{child_alias}.\"{child_pk}\" = {parent_alias}.\"{fk_col}\""
        )),
        RelationshipKind::ManyToMany => {
            Err("m2m relationships use join_predicate_m2m_*, not join_predicate_direct".into())
        }
    }
}

/// Through-row → parent PK predicate for m2m joins.
pub(crate) fn join_predicate_m2m_parent(
    through_alias: &str,
    source_join_col: &str,
    parent_alias: &str,
    parent_pk: &str,
) -> String {
    format!("{through_alias}.\"{source_join_col}\" = {parent_alias}.\"{parent_pk}\"")
}

/// Through-row → target PK ON-clause fragment for m2m joins.
pub(crate) fn join_predicate_m2m_target(
    through_alias: &str,
    target_fk: &str,
    child_alias: &str,
    child_pk: &str,
) -> String {
    format!("{through_alias}.\"{target_fk}\" = {child_alias}.\"{child_pk}\"")
}

#[derive(Clone, Debug)]
pub struct SqlPlan {
    pub sql: String,
    pub binds: Vec<BindValue>,
    /// JSON paths (dot-separated response keys) that need hex→base64 rewrite (SQLite Bytes).
    pub bytes_hex_paths: Vec<String>,
    pub tables_touched: Vec<String>,
    /// Compiler-owned shape for recovering every causal row identity. The
    /// hidden SQL aliases it describes are stripped before GraphQL sees data.
    pub(crate) evidence: QueryEvidencePlan,
}

impl SqlPlan {
    /// Recover complete physical keys and remove all compiler-only aliases.
    ///
    /// Call this after dialect JSON normalization (including SQLite's
    /// hex-to-base64 rewrite) and before converting to an async-graphql value.
    /// Shape errors still perform every safe, plan-guided removal so internal
    /// identity fields are never disclosed through an error path.
    pub(crate) fn extract_evidence_and_strip(
        &self,
        value: &mut JsonValue,
    ) -> Result<ExtractedQueryEvidence, String> {
        self.evidence.extract_and_strip(value)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct QueryEvidencePlan {
    root_response_key: String,
    root: QueryEvidenceNode,
}

#[derive(Clone, Debug)]
enum QueryEvidenceNode {
    Object(QueryEvidenceObjectPlan),
    List(Box<QueryEvidenceNode>),
}

#[derive(Clone, Debug)]
struct QueryEvidenceObjectPlan {
    record: Option<QueryEvidenceRecordPlan>,
    fields: Vec<QueryEvidenceFieldPlan>,
}

#[derive(Clone, Debug)]
struct QueryEvidenceRecordPlan {
    model: String,
    key_fields: Vec<QueryEvidenceKeyPlan>,
}

#[derive(Clone, Debug)]
struct QueryEvidenceKeyPlan {
    hidden_key: String,
    column: String,
}

#[derive(Clone, Debug)]
struct QueryEvidenceFieldPlan {
    /// Key present in the compiler-owned SQL JSON object. Projection storage is
    /// response-keyed so repeated selections of one schema field remain
    /// distinct when their arguments or nested selections differ.
    storage_key: String,
    /// Key emitted in the final GraphQL response and therefore used in causal
    /// record paths.
    response_key: String,
    node: Box<QueryEvidenceNode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QueryResponsePathSegment {
    Field(String),
    Index(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QueryRecordEvidence {
    pub(crate) model: String,
    pub(crate) key_columns: BTreeMap<String, JsonValue>,
    pub(crate) response_path: Vec<QueryResponsePathSegment>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExtractedQueryEvidence {
    pub(crate) records: Vec<QueryRecordEvidence>,
    /// False means the bounded collector saw more records than it can safely
    /// retain. Hidden fields were still removed, but callers must use their
    /// conservative causal fallback instead of partial evidence.
    pub(crate) complete: bool,
}

#[derive(Default)]
struct QueryEvidencePlanSize {
    nodes: usize,
    key_fields: usize,
}

#[derive(Default)]
struct QueryEvidenceExtraction {
    records: Vec<QueryRecordEvidence>,
    records_seen: usize,
    overflowed: bool,
    first_error: Option<String>,
}

impl QueryEvidencePlan {
    fn new(root_response_key: String, root: QueryEvidenceNode) -> Result<Self, String> {
        validate_response_key(&root_response_key)?;
        let mut size = QueryEvidencePlanSize::default();
        root.validate(&mut size)?;
        Ok(Self {
            root_response_key,
            root,
        })
    }

    fn extract_and_strip(&self, value: &mut JsonValue) -> Result<ExtractedQueryEvidence, String> {
        let mut extraction = QueryEvidenceExtraction::default();
        let mut path = vec![QueryResponsePathSegment::Field(
            self.root_response_key.clone(),
        )];
        self.root.visit_and_strip(value, &mut path, &mut extraction);

        if let Some(error) = extraction.first_error {
            return Err(error);
        }
        if extraction.overflowed {
            extraction.records.clear();
        }
        Ok(ExtractedQueryEvidence {
            records: extraction.records,
            complete: !extraction.overflowed,
        })
    }
}

impl QueryEvidenceNode {
    fn validate(&self, size: &mut QueryEvidencePlanSize) -> Result<(), String> {
        size.nodes = size
            .nodes
            .checked_add(1)
            .ok_or_else(|| "query causal-evidence plan size overflowed".to_string())?;
        if size.nodes > MAX_QUERY_EVIDENCE_NODES {
            return Err(format!(
                "query causal-evidence plan exceeds {MAX_QUERY_EVIDENCE_NODES} nodes"
            ));
        }

        match self {
            Self::List(item) => item.validate(size),
            Self::Object(object) => {
                if let Some(record) = &object.record {
                    if record.model.trim().is_empty() || record.key_fields.is_empty() {
                        return Err(
                            "query causal-evidence record has no model or primary key".into()
                        );
                    }
                    size.key_fields = size
                        .key_fields
                        .checked_add(record.key_fields.len())
                        .ok_or_else(|| {
                            "query causal-evidence key-field count overflowed".to_string()
                        })?;
                    if size.key_fields > MAX_QUERY_EVIDENCE_KEY_FIELDS {
                        return Err(format!(
                            "query causal-evidence plan exceeds {MAX_QUERY_EVIDENCE_KEY_FIELDS} key fields"
                        ));
                    }
                    let mut hidden_keys = std::collections::BTreeSet::new();
                    let mut columns = std::collections::BTreeSet::new();
                    for key in &record.key_fields {
                        if !key.hidden_key.starts_with(QUERY_EVIDENCE_HIDDEN_PREFIX)
                            || is_valid_graphql_name(&key.hidden_key)
                            || key.column.trim().is_empty()
                            || !hidden_keys.insert(key.hidden_key.as_str())
                            || !columns.insert(key.column.as_str())
                        {
                            return Err(
                                "query causal-evidence record has an invalid or duplicate key field"
                                    .into(),
                            );
                        }
                    }
                }

                let mut response_keys = std::collections::BTreeSet::new();
                for field in &object.fields {
                    validate_response_key(&field.storage_key)?;
                    validate_response_key(&field.response_key)?;
                    if !response_keys.insert(field.response_key.as_str()) {
                        return Err(format!(
                            "query causal-evidence object repeats response key `{}`",
                            field.response_key
                        ));
                    }
                    field.node.validate(size)?;
                }
                Ok(())
            }
        }
    }

    fn visit_and_strip(
        &self,
        value: &mut JsonValue,
        path: &mut Vec<QueryResponsePathSegment>,
        extraction: &mut QueryEvidenceExtraction,
    ) {
        match self {
            Self::List(item) => match value {
                JsonValue::Null => {}
                JsonValue::Array(items) => {
                    for (index, value) in items.iter_mut().enumerate() {
                        path.push(QueryResponsePathSegment::Index(index));
                        item.visit_and_strip(value, path, extraction);
                        path.pop();
                    }
                }
                // Walk the expected item shape as a defensive cleanup even
                // when the database returned an impossible shape.
                other => {
                    extraction.record_error(format!(
                        "query causal-evidence expected a list at {}",
                        format_response_path(path)
                    ));
                    item.visit_and_strip(other, path, extraction);
                }
            },
            Self::Object(object) => match value {
                JsonValue::Null => {}
                JsonValue::Object(map) => object.visit_and_strip(map, path, extraction),
                JsonValue::Array(items) => {
                    extraction.record_error(format!(
                        "query causal-evidence expected an object at {}",
                        format_response_path(path)
                    ));
                    for (index, value) in items.iter_mut().enumerate() {
                        path.push(QueryResponsePathSegment::Index(index));
                        if let JsonValue::Object(map) = value {
                            object.visit_and_strip(map, path, extraction);
                        }
                        path.pop();
                    }
                }
                _ => extraction.record_error(format!(
                    "query causal-evidence expected an object at {}",
                    format_response_path(path)
                )),
            },
        }
    }
}

impl QueryEvidenceObjectPlan {
    fn visit_and_strip(
        &self,
        map: &mut serde_json::Map<String, JsonValue>,
        path: &mut Vec<QueryResponsePathSegment>,
        extraction: &mut QueryEvidenceExtraction,
    ) {
        if let Some(record) = &self.record {
            let mut key_columns = BTreeMap::new();
            let mut complete_key = true;
            for key in &record.key_fields {
                match map.remove(&key.hidden_key) {
                    Some(value) => {
                        key_columns.insert(key.column.clone(), value);
                    }
                    None => {
                        complete_key = false;
                        extraction.record_error(format!(
                            "query causal-evidence is missing key column `{}` for model `{}` at {}",
                            key.column,
                            record.model,
                            format_response_path(path)
                        ));
                    }
                }
            }
            if complete_key {
                extraction.records_seen += 1;
                if extraction.records_seen <= MAX_QUERY_EVIDENCE_RECORDS {
                    extraction.records.push(QueryRecordEvidence {
                        model: record.model.clone(),
                        key_columns,
                        response_path: path.clone(),
                    });
                } else {
                    extraction.overflowed = true;
                }
            }
        }

        // A newer compiler or malformed row must not leak an unrecognized
        // reserved alias. This only examines record/container objects selected
        // by the evidence tree; arbitrary user JSON values are never walked.
        let unexpected_hidden = map
            .keys()
            .filter(|key| key.starts_with(QUERY_EVIDENCE_HIDDEN_PREFIX))
            .cloned()
            .collect::<Vec<_>>();
        for key in unexpected_hidden {
            map.remove(&key);
            extraction.record_error(format!(
                "query causal-evidence contained unexpected hidden field `{key}` at {}",
                format_response_path(path)
            ));
        }

        for field in &self.fields {
            path.push(QueryResponsePathSegment::Field(field.response_key.clone()));
            match map.get_mut(&field.storage_key) {
                Some(value) => field.node.visit_and_strip(value, path, extraction),
                None => extraction.record_error(format!(
                    "query causal-evidence is missing storage field `{}` for response field `{}` at {}",
                    field.storage_key,
                    field.response_key,
                    format_response_path(path)
                )),
            }
            path.pop();
        }
    }
}

impl QueryEvidenceExtraction {
    fn record_error(&mut self, error: String) {
        if self.first_error.is_none() {
            self.first_error = Some(error);
        }
    }
}

fn format_response_path(path: &[QueryResponsePathSegment]) -> String {
    let mut rendered = String::from("$");
    for segment in path {
        match segment {
            QueryResponsePathSegment::Field(field) => {
                rendered.push('.');
                rendered.push_str(field);
            }
            QueryResponsePathSegment::Index(index) => {
                rendered.push('[');
                rendered.push_str(&index.to_string());
                rendered.push(']');
            }
        }
    }
    rendered
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
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
    // Relationship-aware complexity before SQL (covers query + subscription paths).
    let cost = super::complexity::estimate_root_complexity(inner, model_name, kind, selection)?;
    if super::complexity::exceeds_budget(cost, inner.max_complexity) {
        return Err(format!(
            "query too complex (estimated {cost}, max {})",
            inner.max_complexity
        ));
    }

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

    let order_sql = compile_order_by(
        &entry.schema,
        selection.args.get("order_by"),
        alias,
        perm,
        inner.strict_where,
    )?;

    let ops = inner.dialect.ops();

    let (sql, evidence_root) = match kind {
        RootKind::List => {
            let (projection, object_evidence) = compile_object_projection(
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
            let json_agg = ops.json_agg;
            let coalesce_empty = ops.empty_array;
            let agg_arg = match ops.json_cast_fn {
                None => "root".to_string(),
                Some(f) => format!("{f}(root)"),
            };
            (
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
                ),
                QueryEvidenceNode::List(Box::new(QueryEvidenceNode::Object(object_evidence))),
            )
        }
        RootKind::ByPk => {
            // Projection first: nested has_many/m2m subqueries emit LIMIT/OFFSET
            // `?` binds that appear in the SELECT text *before* the outer WHERE.
            // SQLite binds are positional, so PK + filter binds must be pushed
            // after projection binds (same order as `?` appearance in SQL).
            let (projection, object_evidence) = compile_object_projection(
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
            let mut pk_preds = Vec::new();
            for pk in &entry.schema.primary_key.columns {
                let v = selection
                    .args
                    .get(pk)
                    .ok_or_else(|| format!("missing primary key argument `{pk}`"))?;
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
            let pk_where = pk_preds.join(" AND ");
            let full_where = if where_sql == "TRUE" || where_sql == "true" {
                pk_where
            } else {
                format!("({pk_where}) AND ({where_sql})")
            };
            (
                format!(
                    "SELECT {projection} FROM \"{}\" {alias} WHERE {full_where} LIMIT 1",
                    entry.schema.table_name
                ),
                QueryEvidenceNode::Object(object_evidence),
            )
        }
        RootKind::Aggregate => {
            let json_agg = ops.json_agg;
            let coalesce_empty = ops.empty_array;
            let table = entry.schema.table_name.as_str();
            let mut pairs = Vec::new();
            let mut evidence_fields = Vec::new();
            for aggregate_member in &selection.children {
                match aggregate_member.field_name.as_str() {
                    "__typename" => {}
                    "aggregate" => {
                        validate_response_key(&aggregate_member.response_key)?;
                        let mut aggregate_pairs = Vec::new();
                        for metric in &aggregate_member.children {
                            match metric.field_name.as_str() {
                                "__typename" => {}
                                "count" => {
                                    validate_response_key(&metric.response_key)?;
                                    let where_for_count = compile_where(
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
                                    aggregate_pairs.push((
                                        metric.response_key.clone(),
                                        format!(
                                            "(SELECT count(*) FROM \"{table}\" {alias} WHERE {where_for_count})"
                                        ),
                                    ));
                                }
                                _ => {
                                    return Err(
                                        "aggregate fields selection contains an unsupported member"
                                            .into(),
                                    );
                                }
                            }
                        }
                        pairs.push((
                            aggregate_member.response_key.clone(),
                            chunked_json_object(inner.dialect, &aggregate_pairs),
                        ));
                    }
                    "nodes" => {
                        validate_response_key(&aggregate_member.response_key)?;
                        let nodes_path = aggregate_member.response_key.as_str();
                        let (nodes_proj, nodes_evidence) = compile_object_projection(
                            inner,
                            session,
                            role,
                            &entry.schema,
                            perm,
                            aggregate_member,
                            alias,
                            &mut binds,
                            &mut bytes_paths,
                            &mut tables,
                            nodes_path,
                            0,
                        )?;
                        let where_for_nodes = compile_where(
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
                        let lim = {
                            binds.push(BindValue::I64(limit as i64));
                            placeholder(inner.dialect, binds.len())
                        };
                        let off = {
                            binds.push(BindValue::I64(offset as i64));
                            placeholder(inner.dialect, binds.len())
                        };
                        pairs.push((
                            aggregate_member.response_key.clone(),
                            format!(
                                "coalesce((SELECT {json_agg}(n) FROM (SELECT {nodes_proj} AS n FROM \"{table}\" {alias} WHERE {where_for_nodes} {order_sql} LIMIT {lim} OFFSET {off}) x), {coalesce_empty})"
                            ),
                        ));
                        evidence_fields.push(QueryEvidenceFieldPlan {
                            storage_key: aggregate_member.response_key.clone(),
                            response_key: aggregate_member.response_key.clone(),
                            node: Box::new(QueryEvidenceNode::List(Box::new(
                                QueryEvidenceNode::Object(nodes_evidence),
                            ))),
                        });
                    }
                    _ => {
                        return Err("aggregate selection contains an unsupported member".into());
                    }
                }
            }

            (
                format!("SELECT {}", chunked_json_object(inner.dialect, &pairs)),
                QueryEvidenceNode::Object(QueryEvidenceObjectPlan {
                    record: None,
                    fields: evidence_fields,
                }),
            )
        }
    };
    let evidence = QueryEvidencePlan::new(selection.response_key.clone(), evidence_root)?;

    Ok(SqlPlan {
        sql,
        binds,
        bytes_hex_paths: bytes_paths,
        tables_touched: tables,
        evidence,
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
        Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_i64().and_then(|i| u64::try_from(i).ok())),
        _ => None,
    }
}

fn compile_object_projection(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    schema: &TableSchema,
    perm: &ReadPermission,
    selection: &SelectionNode,
    alias: &str,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    tables: &mut Vec<String>,
    path_prefix: &str,
    depth: usize,
) -> Result<(String, QueryEvidenceObjectPlan), String> {
    if depth > inner.max_depth {
        return Err("max depth exceeded".into());
    }
    let (mut pairs, record) = compile_record_evidence_projection(
        inner.dialect,
        schema,
        perm,
        alias,
        binds,
        bytes_paths,
        path_prefix,
    )?;
    let mut evidence_fields = Vec::new();

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
            if let Some(rel_name) = child.field_name.strip_suffix("_aggregate") {
                if let Some(rel) = schema
                    .relationships
                    .iter()
                    .find(|r| r.field_name == rel_name)
                {
                    let target_entry = match inner.catalog.get(&rel.target_model) {
                        Some(e) => e,
                        None => continue,
                    };
                    let target_perm = match inner
                        .permissions
                        .get(&(rel.target_model.clone(), role.to_string()))
                    {
                        Some(p) if p.permission.aggregations => &p.permission,
                        _ => continue,
                    };
                    tables.push(target_entry.schema.table_name.clone());
                    let child_path = if path_prefix.is_empty() {
                        child.response_key.clone()
                    } else {
                        format!("{path_prefix}.{}", child.response_key)
                    };
                    let (sub, evidence_node) = compile_relationship_aggregate_subquery(
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
                    evidence_fields.push(QueryEvidenceFieldPlan {
                        storage_key: child.response_key.clone(),
                        response_key: child.response_key.clone(),
                        node: Box::new(evidence_node),
                    });
                }
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
                let (sub, evidence_node) = compile_relationship_subquery(
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
                evidence_fields.push(QueryEvidenceFieldPlan {
                    storage_key: child.response_key.clone(),
                    response_key: child.response_key.clone(),
                    node: Box::new(evidence_node),
                });
            }
        }
    }

    Ok((
        chunked_json_object(inner.dialect, &pairs),
        QueryEvidenceObjectPlan {
            record,
            fields: evidence_fields,
        },
    ))
}

fn compile_record_evidence_projection(
    dialect: SqlDialect,
    schema: &TableSchema,
    perm: &ReadPermission,
    alias: &str,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    path_prefix: &str,
) -> Result<(Vec<(String, String)>, Option<QueryEvidenceRecordPlan>), String> {
    // Embedded client models deliberately have no stable normalized identity.
    // Do not manufacture per-record evidence that the client cannot address;
    // table/projector dependencies still flow through `tables_touched` and
    // produce conservative index evidence.
    if !has_client_normalized_identity(schema, perm) {
        return Ok((Vec::new(), None));
    }

    let mut pairs = Vec::with_capacity(schema.primary_key.columns.len());
    let mut key_fields = Vec::with_capacity(schema.primary_key.columns.len());

    for (ordinal, column_name) in schema.primary_key.columns.iter().enumerate() {
        let column = schema
            .columns
            .iter()
            .find(|column| column.column_name == *column_name)
            .ok_or_else(|| {
                format!(
                    "primary key column `{column_name}` missing from model `{}`",
                    schema.model_name
                )
            })?;
        let hidden_key = format!("{QUERY_EVIDENCE_HIDDEN_PREFIX}{ordinal}");
        debug_assert!(!is_valid_graphql_name(&hidden_key));

        // GraphQL BigInt uses decimal strings. Casting the private identity
        // copy avoids any JSON-number precision loss while leaving the visible
        // field's legacy representation unchanged.
        let expression = match (&column.column_type, dialect) {
            (ColumnType::Integer | ColumnType::UnsignedInteger, SqlDialect::Postgres) => {
                format!("{alias}.\"{}\"::text", column.column_name)
            }
            (ColumnType::Integer | ColumnType::UnsignedInteger, SqlDialect::Sqlite) => {
                format!("CAST({alias}.\"{}\" AS TEXT)", column.column_name)
            }
            // PostgreSQL's MIME-style base64 encoder inserts line breaks for
            // long values. Evidence uses canonical RFC 4648 text so the scope
            // codec can reject ambiguous spellings without rejecting valid
            // byte primary keys.
            (ColumnType::Bytes, SqlDialect::Postgres) => format!(
                "replace(encode({alias}.\"{}\", 'base64'), E'\\n', '')",
                column.column_name
            ),
            _ => column_json_expr(dialect, alias, column, binds)?,
        };
        if matches!(column.column_type, ColumnType::Bytes) && matches!(dialect, SqlDialect::Sqlite)
        {
            bytes_paths.push(if path_prefix.is_empty() {
                hidden_key.clone()
            } else {
                format!("{path_prefix}.{hidden_key}")
            });
        }
        pairs.push((hidden_key.clone(), expression));
        key_fields.push(QueryEvidenceKeyPlan {
            hidden_key,
            column: column.column_name.clone(),
        });
    }

    Ok((
        pairs,
        Some(QueryEvidenceRecordPlan {
            model: schema.model_name.clone(),
            key_fields,
        }),
    ))
}

fn has_client_normalized_identity(schema: &TableSchema, perm: &ReadPermission) -> bool {
    !schema.primary_key.columns.is_empty()
        && schema.primary_key.columns.iter().all(|key| {
            schema
                .columns
                .iter()
                .find(|column| column.column_name == *key)
                .is_some_and(|column| {
                    !column.skipped
                        && !column.nullable
                        && perm.allows_column(key)
                        && scalar_type_name(&column.column_type)
                            .is_some_and(|scalar| scalar != "BigInt")
                })
        })
}

fn compile_relationship_aggregate_subquery(
    inner: &EngineInner,
    session: &Session,
    role: &str,
    source: &TableSchema,
    source_alias: &str,
    rel: &crate::table::RelationshipDef,
    target: &CatalogEntry,
    target_perm: &ReadPermission,
    selection: &SelectionNode,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    tables: &mut Vec<String>,
    path_prefix: &str,
    depth: usize,
) -> Result<(String, QueryEvidenceNode), String> {
    let child_alias = format!("ta{depth}");
    let fk = rel.foreign_key.as_deref().unwrap_or("");
    let source_pk = source
        .primary_key
        .columns
        .first()
        .map(|s| s.as_str())
        .unwrap_or("id");

    let (from_sql, join_pred) = match rel.kind {
        RelationshipKind::HasMany => {
            let target_fk = column_name_for(&target.schema, fk).unwrap_or(fk);
            (
                format!("\"{}\" {child_alias}", target.schema.table_name),
                join_predicate_direct(
                    RelationshipKind::HasMany,
                    source_alias,
                    &child_alias,
                    source_pk,
                    /* child_pk unused for has_many */ "",
                    target_fk,
                )?,
            )
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
            let target_fk =
                resolve_m2m_target_foreign_key(source, rel, &through_model.schema, &target.schema)
                    .map_err(|e| e.to_string())?;
            let source_join_col = column_name_for(&through_model.schema, fk).unwrap_or(fk);
            let target_pk = target
                .schema
                .primary_key
                .columns
                .first()
                .map(|s| s.as_str())
                .unwrap_or("id");
            let join_alias = format!("ja{depth}");
            tables.push(through_name.to_string());
            let on_target =
                join_predicate_m2m_target(&join_alias, &target_fk, &child_alias, target_pk);
            (
                format!(
                    "\"{}\" {child_alias} JOIN \"{through_name}\" {join_alias} ON {on_target}",
                    target.schema.table_name
                ),
                join_predicate_m2m_parent(&join_alias, source_join_col, source_alias, source_pk),
            )
        }
        RelationshipKind::BelongsTo => {
            return Err("belongs_to aggregate is not supported".into());
        }
    };

    let ops = inner.dialect.ops();
    let json_agg = ops.json_agg;
    let coalesce_empty = ops.empty_array;
    let mut pairs = Vec::new();
    let mut evidence_fields = Vec::new();
    for aggregate_member in &selection.children {
        match aggregate_member.field_name.as_str() {
            "__typename" => {}
            "aggregate" => {
                validate_response_key(&aggregate_member.response_key)?;
                let mut aggregate_pairs = Vec::new();
                for metric in &aggregate_member.children {
                    match metric.field_name.as_str() {
                        "__typename" => {}
                        "count" => {
                            validate_response_key(&metric.response_key)?;
                            let where_for_count = compile_where(
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
                            aggregate_pairs.push((
                                metric.response_key.clone(),
                                format!(
                                    "(SELECT count(*) FROM {from_sql} WHERE {join_pred} AND ({where_for_count}))"
                                ),
                            ));
                        }
                        _ => {
                            return Err(
                                "relationship aggregate fields selection contains an unsupported member"
                                    .into(),
                            );
                        }
                    }
                }
                pairs.push((
                    aggregate_member.response_key.clone(),
                    chunked_json_object(inner.dialect, &aggregate_pairs),
                ));
            }
            "nodes" => {
                validate_response_key(&aggregate_member.response_key)?;
                let nodes_path = if path_prefix.is_empty() {
                    aggregate_member.response_key.clone()
                } else {
                    format!("{path_prefix}.{}", aggregate_member.response_key)
                };
                let (nodes_proj, nodes_evidence) = compile_object_projection(
                    inner,
                    session,
                    role,
                    &target.schema,
                    target_perm,
                    aggregate_member,
                    &child_alias,
                    binds,
                    bytes_paths,
                    tables,
                    &nodes_path,
                    depth,
                )?;
                let where_for_nodes = compile_where(
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
                    inner.strict_where,
                )?;
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
                let lim = {
                    binds.push(BindValue::I64(limit as i64));
                    placeholder(inner.dialect, binds.len())
                };
                let off = {
                    binds.push(BindValue::I64(offset as i64));
                    placeholder(inner.dialect, binds.len())
                };
                pairs.push((
                    aggregate_member.response_key.clone(),
                    format!(
                        "coalesce((SELECT {json_agg}(n) FROM (SELECT {nodes_proj} AS n FROM {from_sql} WHERE {join_pred} AND ({where_for_nodes}) {order_sql} LIMIT {lim} OFFSET {off}) nested_agg_rows), {coalesce_empty})"
                    ),
                ));
                evidence_fields.push(QueryEvidenceFieldPlan {
                    storage_key: aggregate_member.response_key.clone(),
                    response_key: aggregate_member.response_key.clone(),
                    node: Box::new(QueryEvidenceNode::List(Box::new(
                        QueryEvidenceNode::Object(nodes_evidence),
                    ))),
                });
            }
            _ => {
                return Err(
                    "relationship aggregate selection contains an unsupported member".into(),
                );
            }
        }
    }

    Ok((
        chunked_json_object(inner.dialect, &pairs),
        QueryEvidenceNode::Object(QueryEvidenceObjectPlan {
            record: None,
            fields: evidence_fields,
        }),
    ))
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
        (ColumnType::Json, SqlDialect::Postgres) => q.to_string(),
        _ => q,
    })
}

fn chunked_json_object(dialect: SqlDialect, pairs: &[(String, String)]) -> String {
    let build = dialect.ops().build_object;
    if pairs.is_empty() {
        return format!("{build}()");
    }
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
    target_perm: &ReadPermission,
    selection: &SelectionNode,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    tables: &mut Vec<String>,
    path_prefix: &str,
    depth: usize,
) -> Result<(String, QueryEvidenceNode), String> {
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
    let source_pk_col = source
        .primary_key
        .columns
        .first()
        .map(|s| s.as_str())
        .unwrap_or("id");
    let target_pk_col = target
        .schema
        .primary_key
        .columns
        .first()
        .map(|s| s.as_str())
        .unwrap_or("id");

    let join_pred = match &rel.kind {
        RelationshipKind::HasMany => join_predicate_direct(
            RelationshipKind::HasMany,
            source_alias,
            &child_alias,
            source_pk_col,
            target_pk_col,
            target_fk_col,
        )?,
        RelationshipKind::BelongsTo => join_predicate_direct(
            RelationshipKind::BelongsTo,
            source_alias,
            &child_alias,
            source_pk_col,
            target_pk_col,
            fk_col,
        )?,
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
            let target_fk =
                resolve_m2m_target_foreign_key(source, rel, &through_model.schema, &target.schema)
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

    let order_sql = compile_order_by(
        &target.schema,
        selection.args.get("order_by"),
        &child_alias,
        target_perm,
        inner.strict_where,
    )?;
    let (projection, object_evidence) = compile_object_projection(
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

    let ops = inner.dialect.ops();
    let json_agg = ops.json_agg;
    let coalesce_empty = ops.empty_array;

    match rel.kind {
        RelationshipKind::BelongsTo => Ok((
            format!(
                "(SELECT {projection} FROM \"{}\" {child_alias} WHERE {join_pred} AND ({where_extra}) LIMIT 1)",
                target.schema.table_name
            ),
            QueryEvidenceNode::Object(object_evidence),
        )),
        _ => {
            binds.push(BindValue::I64(limit as i64));
            let lim = placeholder(inner.dialect, binds.len());
            binds.push(BindValue::I64(offset as i64));
            let off = placeholder(inner.dialect, binds.len());
            Ok((
                format!(
                    "(SELECT coalesce({json_agg}(obj), {coalesce_empty}) FROM (\n  SELECT {projection} AS obj\n  FROM \"{}\" {child_alias}\n  WHERE {join_pred} AND ({where_extra})\n  {order_sql}\n  LIMIT {lim} OFFSET {off}\n) inner_rows)",
                    target.schema.table_name
                ),
                QueryEvidenceNode::List(Box::new(QueryEvidenceNode::Object(object_evidence))),
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
    target_perm: &ReadPermission,
    selection: &SelectionNode,
    binds: &mut Vec<BindValue>,
    bytes_paths: &mut Vec<String>,
    tables: &mut Vec<String>,
    path_prefix: &str,
    depth: usize,
    limit: u64,
    offset: u64,
) -> Result<(String, QueryEvidenceNode), String> {
    let child_alias = format!("t{depth}");
    let j_alias = format!("j{depth}");
    let order_sql = compile_order_by(
        target_schema,
        selection.args.get("order_by"),
        &child_alias,
        target_perm,
        inner.strict_where,
    )?;
    let (projection, object_evidence) = compile_object_projection(
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
    let ops = inner.dialect.ops();
    let json_agg = ops.json_agg;
    let coalesce_empty = ops.empty_array;
    binds.push(BindValue::I64(limit as i64));
    let lim = placeholder(inner.dialect, binds.len());
    binds.push(BindValue::I64(offset as i64));
    let off = placeholder(inner.dialect, binds.len());
    let on_target = join_predicate_m2m_target(&j_alias, target_fk, &child_alias, target_pk);
    let parent_pred = join_predicate_m2m_parent(&j_alias, source_join_col, source_alias, source_pk);
    Ok((
        format!(
            "(SELECT coalesce({json_agg}(obj), {coalesce_empty}) FROM (\n  SELECT {projection} AS obj\n  FROM \"{target_table}\" {child_alias}\n  JOIN \"{through_name}\" {j_alias} ON {on_target}\n  WHERE {parent_pred}\n    AND ({where_extra})\n  {order_sql}\n  LIMIT {lim} OFFSET {off}\n) x)",
            target_table = target_schema.table_name,
        ),
        QueryEvidenceNode::List(Box::new(QueryEvidenceNode::Object(object_evidence))),
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
    perm: &ReadPermission,
    strict: bool,
) -> Result<String, String> {
    let mut parts = Vec::new();
    if let Some(Value::List(items)) = order_arg {
        for item in items {
            if let Value::Object(map) = item {
                if map.len() > 1 {
                    return Err(
                        "ambiguous order_by entry: use one field per list entry to declare priority"
                            .into(),
                    );
                }
                for (col, dir) in map {
                    if !schema.columns.iter().any(|c| c.column_name == *col) {
                        if strict {
                            return Err(format!("unknown order_by column `{col}`"));
                        }
                        continue;
                    }
                    if !perm.allows_column(col) {
                        if strict {
                            return Err(format!("ungranted order_by column `{col}`"));
                        }
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
    perm: &ReadPermission,
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
    if let Some(filter) = &perm.row_filter {
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
                CmpOp::Ilike => inner.dialect.ops().ilike_op,
                CmpOp::Contains => {
                    require_postgres_json_op(inner.dialect, "_contains")?;
                    "@>"
                }
                CmpOp::ContainedIn => {
                    require_postgres_json_op(inner.dialect, "_contained_in")?;
                    "<@"
                }
                CmpOp::HasKey => {
                    require_postgres_json_op(inner.dialect, "_has_key")?;
                    "?"
                }
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
                    let source_col = schema
                        .primary_key
                        .columns
                        .first()
                        .map(|s| s.as_str())
                        .unwrap_or("id");
                    let join_pred = join_predicate_direct(
                        RelationshipKind::HasMany,
                        alias,
                        &child_alias,
                        source_col,
                        "",
                        target_fk,
                    )?;
                    Ok(format!(
                        "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {join_pred} AND ({inner_pred}))",
                        target.schema.table_name
                    ))
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
                    let join_pred = join_predicate_direct(
                        RelationshipKind::BelongsTo,
                        alias,
                        &child_alias,
                        "",
                        target_pk,
                        source_fk,
                    )?;
                    Ok(format!(
                        "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {join_pred} AND ({inner_pred}))",
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
                    let on_target =
                        join_predicate_m2m_target(&j, &target_fk, &child_alias, target_pk);
                    let source_join_col = column_name_for(&through_entry.schema, fk).unwrap_or(fk);
                    let parent_pred =
                        join_predicate_m2m_parent(&j, source_join_col, alias, source_pk);
                    Ok(format!(
                        "EXISTS (SELECT 1 FROM \"{through}\" {j} JOIN \"{}\" {child_alias} ON {on_target} WHERE {parent_pred} AND ({inner_pred}))",
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
    perm: &ReadPermission,
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
                    if items.len() > inner.max_bool_width {
                        return Err(format!(
                            "_and list length {} exceeds max_bool_width {}",
                            items.len(),
                            inner.max_bool_width
                        ));
                    }
                    for item in items {
                        preds.push(compile_client_where(
                            inner,
                            session,
                            role,
                            schema,
                            perm,
                            item,
                            alias,
                            binds,
                            tables,
                            depth + 1,
                        )?);
                    }
                }
            }
            "_or" => {
                if let Value::List(items) = val {
                    if items.len() > inner.max_bool_width {
                        return Err(format!(
                            "_or list length {} exceeds max_bool_width {}",
                            items.len(),
                            inner.max_bool_width
                        ));
                    }
                    let mut parts = Vec::new();
                    for item in items {
                        parts.push(compile_client_where(
                            inner,
                            session,
                            role,
                            schema,
                            perm,
                            item,
                            alias,
                            binds,
                            tables,
                            depth + 1,
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
                        inner,
                        session,
                        role,
                        schema,
                        perm,
                        val,
                        alias,
                        binds,
                        tables,
                        depth + 1,
                    )?
                ));
            }
            col_name => {
                if let Some(col) = schema.columns.iter().find(|c| c.column_name == *col_name) {
                    if !perm.allows_column(col_name) {
                        if inner.strict_where {
                            return Err(format!("ungranted where column `{col_name}`"));
                        }
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
                        None => {
                            if inner.strict_where {
                                return Err(format!(
                                    "unknown where field `{col_name}` (relationship target missing)"
                                ));
                            }
                            continue;
                        }
                    };
                    tables.push(target.schema.table_name.clone());
                    let target_perm = match inner
                        .permissions
                        .get(&(rel.target_model.clone(), role.to_string()))
                    {
                        Some(p) => &p.permission,
                        None => {
                            if inner.strict_where {
                                return Err(format!("ungranted where relationship `{col_name}`"));
                            }
                            continue;
                        }
                    };
                    let child_alias = format!("cw{depth}");
                    // Entering a relationship is a new target-model access path.
                    // Compile the complete target WHERE so its row policy cannot
                    // be bypassed by a source-model client predicate.
                    let inner_pred = compile_where(
                        inner,
                        session,
                        role,
                        &target.schema,
                        target_perm,
                        Some(val),
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
                            let join_pred = join_predicate_direct(
                                RelationshipKind::HasMany,
                                alias,
                                &child_alias,
                                source_col,
                                "",
                                target_fk,
                            )?;
                            preds.push(format!(
                                "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {join_pred} AND ({inner_pred}))",
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
                            let join_pred = join_predicate_direct(
                                RelationshipKind::BelongsTo,
                                alias,
                                &child_alias,
                                "",
                                target_pk,
                                source_fk,
                            )?;
                            preds.push(format!(
                                "EXISTS (SELECT 1 FROM \"{}\" {child_alias} WHERE {join_pred} AND ({inner_pred}))",
                                target.schema.table_name
                            ));
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
                            let source_join_col =
                                column_name_for(&through_entry.schema, fk).unwrap_or(fk);
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
                            let join_alias = format!("cwj{depth}");
                            tables.push(through.to_string());
                            let on_target = join_predicate_m2m_target(
                                &join_alias,
                                &target_fk,
                                &child_alias,
                                target_pk,
                            );
                            let parent_pred = join_predicate_m2m_parent(
                                &join_alias,
                                source_join_col,
                                alias,
                                source_pk,
                            );
                            preds.push(format!(
                                "EXISTS (SELECT 1 FROM \"{through}\" {join_alias} JOIN \"{}\" {child_alias} ON {on_target} WHERE {parent_pred} AND ({inner_pred}))",
                                target.schema.table_name
                            ));
                        }
                    }
                } else if inner.strict_where {
                    return Err(format!("unknown where field `{col_name}`"));
                }
                // soft-skip: ignore unknown keys when strict_where is false
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
                "_ilike" => inner.dialect.ops().ilike_op,
                "_contains" => {
                    require_postgres_json_op(inner.dialect, "_contains")?;
                    "@>"
                }
                "_contained_in" => {
                    require_postgres_json_op(inner.dialect, "_contained_in")?;
                    "<@"
                }
                "_has_key" => {
                    require_postgres_json_op(inner.dialect, "_has_key")?;
                    "?"
                }
                _ => return Err(format!("unknown comparison op `{other}`")),
            };
            binds.push(value_to_bind(rhs, column_type)?);
            let ph = placeholder(inner.dialect, binds.len());
            let cast_ph = cast_placeholder(inner.dialect, column_type, &ph);
            Ok(format!("{col_ref} {sql_op} {cast_ph}"))
        }
    }
}

/// Postgres jsonb operators are not emitted on SQLite (would confuse the driver
/// and risk opaque execute errors). Message must stay free of SQL fragments so
/// [`super::schema::sanitize_compile_error`] maps to a stable client code.
fn require_postgres_json_op(dialect: SqlDialect, op: &str) -> Result<(), String> {
    match dialect {
        SqlDialect::Postgres => Ok(()),
        SqlDialect::Sqlite => Err(format!(
            "unknown comparison op `{op}` is not supported on sqlite"
        )),
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
#[allow(dead_code)]
pub fn compile_list_sql_for_test(
    dialect: SqlDialect,
    schema: &TableSchema,
    where_sql: &str,
    limit: u64,
) -> String {
    let ops = dialect.ops();
    let json_agg = ops.json_agg;
    let coalesce_empty = ops.empty_array;
    let build = ops.build_object;
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
mod query_evidence_tests {
    use super::*;
    use crate::graphql::permissions::read;
    use crate::table::{PrimaryKey, TableColumn, TableKind};

    fn hidden(ordinal: usize) -> String {
        format!("{QUERY_EVIDENCE_HIDDEN_PREFIX}{ordinal}")
    }

    fn evidence_alias(response_key: &str, node: QueryEvidenceNode) -> QueryEvidenceFieldPlan {
        QueryEvidenceFieldPlan {
            storage_key: response_key.into(),
            response_key: response_key.into(),
            node: Box::new(node),
        }
    }

    fn object_node(
        model: Option<&str>,
        columns: &[&str],
        fields: Vec<QueryEvidenceFieldPlan>,
    ) -> QueryEvidenceNode {
        QueryEvidenceNode::Object(QueryEvidenceObjectPlan {
            record: model.map(|model| QueryEvidenceRecordPlan {
                model: model.into(),
                key_fields: columns
                    .iter()
                    .enumerate()
                    .map(|(ordinal, column)| QueryEvidenceKeyPlan {
                        hidden_key: hidden(ordinal),
                        column: (*column).into(),
                    })
                    .collect(),
            }),
            fields,
        })
    }

    fn list_node(item: QueryEvidenceNode) -> QueryEvidenceNode {
        QueryEvidenceNode::List(Box::new(item))
    }

    fn object(entries: impl IntoIterator<Item = (impl Into<String>, JsonValue)>) -> JsonValue {
        JsonValue::Object(
            entries
                .into_iter()
                .map(|(key, value)| (key.into(), value))
                .collect(),
        )
    }

    #[test]
    fn list_evidence_tracks_aliases_nested_relationships_and_composite_keys() {
        let comment = || object_node(Some("Comment"), &["tenant_id", "comment_id"], Vec::new());
        let plan = QueryEvidencePlan::new(
            "usersAlias".into(),
            list_node(object_node(
                Some("User"),
                &["user_id"],
                vec![
                    evidence_alias(
                        "authorAlias",
                        object_node(Some("Profile"), &["profile_id"], Vec::new()),
                    ),
                    evidence_alias("commentsAlias", list_node(comment())),
                    evidence_alias(
                        "commentsStatsAlias",
                        object_node(
                            None,
                            &[],
                            vec![evidence_alias("rowsAlias", list_node(comment()))],
                        ),
                    ),
                ],
            )),
        )
        .unwrap();

        let metadata = object([(hidden(0), serde_json::json!("user-owned-json"))]);
        let author = object([
            (hidden(0), serde_json::json!("profile-1")),
            ("displayName".into(), serde_json::json!("Pat")),
        ]);
        let first_comment = object([
            (hidden(0), serde_json::json!("tenant-1")),
            (hidden(1), serde_json::json!("9223372036854775807")),
            ("body".into(), serde_json::json!("first")),
        ]);
        let second_comment = object([
            (hidden(0), serde_json::json!("tenant-1")),
            (hidden(1), serde_json::json!("9223372036854775808")),
            ("body".into(), serde_json::json!("second")),
        ]);
        let aggregate_comment = object([
            (hidden(0), serde_json::json!("tenant-1")),
            (hidden(1), serde_json::json!("AP8=")),
            ("body".into(), serde_json::json!("aggregate")),
        ]);
        let mut value = JsonValue::Array(vec![object([
            (hidden(0), serde_json::json!("user-1")),
            ("name".into(), serde_json::json!("User One")),
            ("metadata".into(), metadata.clone()),
            ("authorAlias".into(), author),
            (
                "commentsAlias".into(),
                JsonValue::Array(vec![first_comment, second_comment]),
            ),
            (
                "commentsStatsAlias".into(),
                object([
                    ("aggregate", serde_json::json!({"count": 1})),
                    ("rowsAlias", JsonValue::Array(vec![aggregate_comment])),
                ]),
            ),
        ])]);

        let extracted = plan.extract_and_strip(&mut value).unwrap();
        assert!(extracted.complete);
        assert_eq!(extracted.records.len(), 5);
        assert_eq!(
            extracted.records[0],
            QueryRecordEvidence {
                model: "User".into(),
                key_columns: BTreeMap::from([("user_id".into(), serde_json::json!("user-1"))]),
                response_path: vec![
                    QueryResponsePathSegment::Field("usersAlias".into()),
                    QueryResponsePathSegment::Index(0),
                ],
            }
        );
        assert_eq!(
            extracted.records[2].response_path,
            vec![
                QueryResponsePathSegment::Field("usersAlias".into()),
                QueryResponsePathSegment::Index(0),
                QueryResponsePathSegment::Field("commentsAlias".into()),
                QueryResponsePathSegment::Index(0),
            ]
        );
        assert_eq!(
            extracted.records[2].key_columns,
            BTreeMap::from([
                (
                    "comment_id".into(),
                    serde_json::json!("9223372036854775807")
                ),
                ("tenant_id".into(), serde_json::json!("tenant-1")),
            ])
        );
        assert_eq!(
            extracted.records[4].response_path,
            vec![
                QueryResponsePathSegment::Field("usersAlias".into()),
                QueryResponsePathSegment::Index(0),
                QueryResponsePathSegment::Field("commentsStatsAlias".into()),
                QueryResponsePathSegment::Field("rowsAlias".into()),
                QueryResponsePathSegment::Index(0),
            ]
        );

        let expected = JsonValue::Array(vec![object([
            ("name", serde_json::json!("User One")),
            ("metadata", metadata),
            ("authorAlias", serde_json::json!({"displayName": "Pat"})),
            (
                "commentsAlias",
                serde_json::json!([
                    {"body": "first"},
                    {"body": "second"}
                ]),
            ),
            (
                "commentsStatsAlias",
                serde_json::json!({
                    "aggregate": {"count": 1},
                    "rowsAlias": [{"body": "aggregate"}]
                }),
            ),
        ])]);
        assert_eq!(value, expected);
        assert_eq!(
            value[0]["metadata"][hidden(0).as_str()],
            serde_json::json!("user-owned-json"),
            "plan-guided stripping must not recurse into arbitrary JSON scalars"
        );
    }

    #[test]
    fn by_pk_evidence_uses_the_root_alias_and_null_has_no_record() {
        let plan = QueryEvidencePlan::new(
            "itemAlias".into(),
            object_node(Some("Item"), &["item_id"], Vec::new()),
        )
        .unwrap();
        let mut value = object([
            (hidden(0), serde_json::json!("item-1")),
            ("label".into(), serde_json::json!("one")),
        ]);

        let extracted = plan.extract_and_strip(&mut value).unwrap();
        assert_eq!(
            extracted.records[0].response_path,
            vec![QueryResponsePathSegment::Field("itemAlias".into())]
        );
        assert_eq!(value, serde_json::json!({"label": "one"}));

        let mut absent = JsonValue::Null;
        let extracted = plan.extract_and_strip(&mut absent).unwrap();
        assert!(extracted.complete);
        assert!(extracted.records.is_empty());
    }

    #[test]
    fn hidden_aliases_cannot_collide_and_are_stripped_on_shape_errors() {
        assert!(!is_valid_graphql_name(&hidden(0)));
        let plan = QueryEvidencePlan::new(
            "items".into(),
            list_node(object_node(Some("Item"), &["id"], Vec::new())),
        )
        .unwrap();
        let unexpected = hidden(99);
        let mut value = JsonValue::Array(vec![
            object([
                (hidden(0), serde_json::json!("one")),
                (unexpected, serde_json::json!("private")),
            ]),
            object([("visible", serde_json::json!(true))]),
        ]);

        let error = plan.extract_and_strip(&mut value).unwrap_err();
        assert!(error.contains("unexpected hidden field"), "{error}");
        assert!(value[0].as_object().unwrap().is_empty());
        assert_eq!(value[1], serde_json::json!({"visible": true}));
        assert!(
            !serde_json::to_string(&value)
                .unwrap()
                .contains(QUERY_EVIDENCE_HIDDEN_PREFIX),
            "all record-level hidden aliases must be removed even after the first error"
        );
    }

    #[test]
    fn record_collection_bound_falls_back_without_disclosing_hidden_keys() {
        let plan = QueryEvidencePlan::new(
            "items".into(),
            list_node(object_node(Some("Item"), &["id"], Vec::new())),
        )
        .unwrap();
        let mut value = JsonValue::Array(
            (0..=MAX_QUERY_EVIDENCE_RECORDS)
                .map(|index| object([(hidden(0), serde_json::json!(index))]))
                .collect(),
        );

        let extracted = plan.extract_and_strip(&mut value).unwrap();
        assert!(!extracted.complete);
        assert!(extracted.records.is_empty());
        assert!(value
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value.as_object().unwrap().is_empty()));
    }

    #[test]
    fn evidence_plan_rejects_key_field_and_node_bounds() {
        let too_many_keys = (0..=MAX_QUERY_EVIDENCE_KEY_FIELDS)
            .map(|ordinal| format!("column_{ordinal}"))
            .collect::<Vec<_>>();
        let key_refs = too_many_keys.iter().map(String::as_str).collect::<Vec<_>>();
        let error = QueryEvidencePlan::new(
            "items".into(),
            object_node(Some("Wide"), &key_refs, Vec::new()),
        )
        .unwrap_err();
        assert!(error.contains("key fields"), "{error}");

        let mut deep = object_node(None, &[], Vec::new());
        for _ in 0..MAX_QUERY_EVIDENCE_NODES {
            deep = list_node(deep);
        }
        let error = QueryEvidencePlan::new("items".into(), deep).unwrap_err();
        assert!(error.contains("nodes"), "{error}");
    }

    #[test]
    fn compiler_injects_lossless_private_keys_even_when_not_selected() {
        let schema = TableSchema {
            model_name: "Composite".into(),
            table_name: "composites".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("sequence", "sequence_id", ColumnType::Text)
                },
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("digest", "digest_bytes", ColumnType::Bytes)
                },
                TableColumn::new("visible", "visible", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["sequence_id", "digest_bytes"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let permission = read().all_columns();
        let mut binds = Vec::new();
        let mut bytes_paths = Vec::new();
        let (pairs, record) = compile_record_evidence_projection(
            SqlDialect::Sqlite,
            &schema,
            &permission,
            "t7",
            &mut binds,
            &mut bytes_paths,
            "childrenAlias",
        )
        .unwrap();

        assert_eq!(pairs.len(), 2);
        let record = record.expect("client-normalized identity evidence");
        assert_eq!(record.model, "Composite");
        assert_eq!(record.key_fields[0].column, "sequence_id");
        assert_eq!(pairs[0].0, hidden(0));
        assert_eq!(pairs[0].1, "t7.\"sequence_id\"");
        assert_eq!(pairs[1].0, hidden(1));
        assert!(pairs[1].1.contains("hex"));
        assert_eq!(bytes_paths, vec![format!("childrenAlias.{}", hidden(1))]);
        let sql = chunked_json_object(SqlDialect::Sqlite, &pairs);
        assert!(sql.contains(QUERY_EVIDENCE_HIDDEN_PREFIX), "{sql}");
        assert!(!sql.contains("'visible'"), "{sql}");

        let (postgres_pairs, _) = compile_record_evidence_projection(
            SqlDialect::Postgres,
            &schema,
            &permission,
            "t7",
            &mut Vec::new(),
            &mut Vec::new(),
            "",
        )
        .unwrap();
        assert_eq!(postgres_pairs[0].1, "t7.\"sequence_id\"");
        assert!(
            postgres_pairs[1].1.contains("replace(encode(")
                && postgres_pairs[1].1.contains("E'\\n'"),
            "{}",
            postgres_pairs[1].1
        );
    }

    #[test]
    fn embedded_client_identity_omits_record_projection_but_not_row_data() {
        let schema = TableSchema {
            model_name: "BigIntRecord".into(),
            table_name: "bigint_records".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("sequence", "sequence_id", ColumnType::UnsignedInteger)
                },
                TableColumn::new("visible", "visible", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["sequence_id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let permission = read().all_columns();

        let (pairs, record) = compile_record_evidence_projection(
            SqlDialect::Sqlite,
            &schema,
            &permission,
            "t0",
            &mut Vec::new(),
            &mut Vec::new(),
            "",
        )
        .unwrap();

        assert!(pairs.is_empty());
        assert!(record.is_none());
    }
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

    #[test]
    fn resolve_limit_ignores_negative_values() {
        assert_eq!(resolve_limit(Some(&Value::from(-1)), None, 100, 1000), 100);
        assert_eq!(value_as_u64(&Value::from(-1)), None);
    }
}

#[cfg(test)]
mod strict_order_by_tests {
    use super::*;
    use crate::graphql::permissions::read;
    use crate::table::{ColumnType, PrimaryKey, TableColumn, TableKind, TableSchema};
    use async_graphql::indexmap::IndexMap;
    use async_graphql::Value as GqlValue;

    fn item_schema() -> TableSchema {
        TableSchema {
            model_name: "Item".into(),
            table_name: "items".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn::new("name", "name", ColumnType::Text),
                TableColumn::new("secret", "secret", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    fn order_list(entries: Vec<(&str, &str)>) -> GqlValue {
        let mut items = Vec::new();
        for (col, dir) in entries {
            let mut map = IndexMap::new();
            map.insert(
                async_graphql::Name::new(col),
                GqlValue::Enum(async_graphql::Name::new(dir)),
            );
            items.push(GqlValue::Object(map));
        }
        GqlValue::List(items)
    }

    #[test]
    fn strict_rejects_unknown_order_column() {
        let schema = item_schema();
        let perm = read().all_columns();
        let arg = order_list(vec![("nope", "asc")]);
        let err = compile_order_by(&schema, Some(&arg), "t0", &perm, true).unwrap_err();
        assert!(err.contains("unknown order_by"), "{err}");
    }

    #[test]
    fn strict_rejects_ungranted_order_column() {
        let schema = item_schema();
        let perm = read().columns(["id", "name"]);
        let arg = order_list(vec![("secret", "asc")]);
        let err = compile_order_by(&schema, Some(&arg), "t0", &perm, true).unwrap_err();
        assert!(err.contains("ungranted order_by"), "{err}");
    }

    #[test]
    fn soft_skip_ignores_unknown_and_ungranted_order() {
        let schema = item_schema();
        let perm = read().columns(["id", "name"]);
        let arg = order_list(vec![("secret", "asc"), ("nope", "desc"), ("name", "desc")]);
        let sql = compile_order_by(&schema, Some(&arg), "t0", &perm, false).unwrap();
        assert!(sql.contains(r#"t0."name" DESC"#), "{sql}");
        assert!(!sql.contains("secret"), "{sql}");
        assert!(!sql.contains("nope"), "{sql}");
        assert!(sql.contains(r#"t0."id" ASC"#), "pk tiebreak: {sql}");
    }

    #[test]
    fn strict_accepts_granted_order_with_pk_tiebreak() {
        let schema = item_schema();
        let perm = read().all_columns();
        let arg = order_list(vec![("name", "desc")]);
        let sql = compile_order_by(&schema, Some(&arg), "t0", &perm, true).unwrap();
        assert!(sql.contains(r#"t0."name" DESC"#), "{sql}");
        assert!(sql.contains(r#"t0."id" ASC"#), "{sql}");
    }

    #[test]
    fn multi_field_order_object_is_rejected_even_in_soft_mode() {
        let schema = item_schema();
        let perm = read().all_columns();
        let mut entry = IndexMap::new();
        entry.insert(
            async_graphql::Name::new("name"),
            GqlValue::Enum(async_graphql::Name::new("desc")),
        );
        entry.insert(
            async_graphql::Name::new("id"),
            GqlValue::Enum(async_graphql::Name::new("asc")),
        );
        let arg = GqlValue::List(vec![GqlValue::Object(entry)]);

        let error = compile_order_by(&schema, Some(&arg), "t0", &perm, false).unwrap_err();
        assert!(error.contains("ambiguous order_by"), "{error}");
        assert!(error.contains("one field per list entry"), "{error}");
    }

    #[test]
    fn separate_order_entries_preserve_declared_priority() {
        let schema = item_schema();
        let perm = read().all_columns();
        let arg = order_list(vec![("name", "desc"), ("secret", "asc")]);

        let sql = compile_order_by(&schema, Some(&arg), "t0", &perm, true).unwrap();
        let name_position = sql.find(r#"t0."name" DESC"#).expect("name ordering");
        let secret_position = sql.find(r#"t0."secret" ASC"#).expect("secret ordering");
        assert!(name_position < secret_position, "{sql}");
    }
}

#[cfg(test)]
mod dialect_ops_tests {
    use super::*;

    #[test]
    fn postgres_ops_table() {
        let ops = SqlDialect::Postgres.ops();
        assert_eq!(ops.json_agg, "jsonb_agg");
        assert_eq!(ops.empty_array, "'[]'::jsonb");
        assert_eq!(ops.build_object, "jsonb_build_object");
        assert_eq!(ops.json_cast_fn, None);
        assert_eq!(ops.ilike_op, "ILIKE");
        assert_eq!(placeholder(SqlDialect::Postgres, 3), "$3");
    }

    #[test]
    fn sqlite_ops_table() {
        let ops = SqlDialect::Sqlite.ops();
        assert_eq!(ops.json_agg, "json_group_array");
        assert_eq!(ops.empty_array, "'[]'");
        assert_eq!(ops.build_object, "json_object");
        assert_eq!(ops.json_cast_fn, Some("json"));
        assert_eq!(ops.ilike_op, "LIKE");
        assert_eq!(placeholder(SqlDialect::Sqlite, 1), "?");
    }
}

#[cfg(test)]
mod join_predicate_tests {
    use super::*;

    #[test]
    fn has_many_join() {
        let sql = join_predicate_direct(
            RelationshipKind::HasMany,
            "t0",
            "t1",
            "order_id",
            "line_id",
            "order_id",
        )
        .unwrap();
        assert_eq!(sql, r#"t1."order_id" = t0."order_id""#);
    }

    #[test]
    fn belongs_to_join() {
        let sql = join_predicate_direct(
            RelationshipKind::BelongsTo,
            "t0",
            "t1",
            "line_id",
            "customer_id",
            "customer_id",
        )
        .unwrap();
        assert_eq!(sql, r#"t1."customer_id" = t0."customer_id""#);
    }

    #[test]
    fn m2m_rejects_direct_helper() {
        let err = join_predicate_direct(RelationshipKind::ManyToMany, "t0", "t1", "a", "b", "c")
            .unwrap_err();
        assert!(err.contains("m2m"), "{err}");
    }

    #[test]
    fn m2m_fragments() {
        assert_eq!(
            join_predicate_m2m_target("j1", "post_id", "t1", "id"),
            r#"j1."post_id" = t1."id""#
        );
        assert_eq!(
            join_predicate_m2m_parent("j1", "user_id", "t0", "id"),
            r#"j1."user_id" = t0."id""#
        );
    }
}

#[cfg(test)]
mod parse_claim_tests {
    use super::*;

    #[test]
    fn integer_claim_ok_and_fail() {
        assert!(matches!(
            parse_claim("42", &ColumnType::Integer).unwrap(),
            BindValue::I64(42)
        ));
        assert!(parse_claim("nope", &ColumnType::Integer).is_err());
    }

    #[test]
    fn bool_claim_variants() {
        assert!(matches!(
            parse_claim("true", &ColumnType::Boolean).unwrap(),
            BindValue::Bool(true)
        ));
        assert!(matches!(
            parse_claim("0", &ColumnType::Boolean).unwrap(),
            BindValue::Bool(false)
        ));
        assert!(parse_claim("maybe", &ColumnType::Boolean).is_err());
    }

    #[test]
    fn json_claim_rejected() {
        assert!(parse_claim("{}", &ColumnType::Json).is_err());
    }
}
