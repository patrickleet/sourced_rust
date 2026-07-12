//! GraphqlEngine builder, validation, and execute entrypoint.
#![allow(clippy::items_after_test_module)]

use std::collections::{btree_map::Entry, BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use async_graphql::{Request, Response, ServerError, Value};
use futures_util::stream::{self, BoxStream};
use futures_util::StreamExt;

use crate::manifest::DistributedProjectManifest;
use crate::microsvc::Session;
use crate::read_model::{ReadModelChange, RelationalReadModelIncludes};
use crate::table::{
    resolve_m2m_target_foreign_key, ColumnType, RelationshipKind, TableKind, TableSchema,
};

use super::commands::GraphqlCommands;
use super::compile::{SqlDialect, SqlPlan};
use super::execute;
use super::filter::{FilterExpr, Operand};
use super::naming::{
    by_pk_field, is_valid_graphql_name, object_type_name, reserved_type_names, root_list_field,
};
use super::permissions::{select, ModelPermissions, SelectPermission};
use super::schema as dyn_schema;
use super::sdl::{graphql_sdl_for_tables_with_options, SdlOptions};

#[derive(Clone)]
pub enum GraphqlPool {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::SqlitePool),
}

#[cfg(feature = "postgres")]
impl From<sqlx::PgPool> for GraphqlPool {
    fn from(pool: sqlx::PgPool) -> Self {
        GraphqlPool::Postgres(pool)
    }
}

#[cfg(feature = "sqlite")]
impl From<sqlx::SqlitePool> for GraphqlPool {
    fn from(pool: sqlx::SqlitePool) -> Self {
        GraphqlPool::Sqlite(pool)
    }
}

#[derive(Debug)]
pub struct GraphqlBuildError(pub String);

impl std::fmt::Display for GraphqlBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GraphqlBuildError {}

impl From<String> for GraphqlBuildError {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for GraphqlBuildError {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct CatalogEntry {
    pub schema: TableSchema,
    pub exposed: bool,
}

#[derive(Clone)]
pub(crate) struct RoleModelPerm {
    pub permission: SelectPermission,
}

pub(crate) struct EngineInner {
    pub pool: GraphqlPool,
    pub catalog: BTreeMap<String, CatalogEntry>,
    pub by_table: BTreeMap<String, String>,
    pub permissions: BTreeMap<(String, String), RoleModelPerm>,
    pub roles: BTreeSet<String>,
    pub anonymous_role: String,
    pub default_limit: u64,
    pub max_limit: u64,
    pub max_depth: usize,
    #[allow(dead_code)]
    pub max_complexity: usize,
    pub max_in_list: usize,
    /// Max length of a single `_and` / `_or` list in client `where` (breadth DoS).
    pub max_bool_width: usize,
    #[allow(dead_code)]
    pub introspection_for_anonymous: bool,
    pub statement_timeout: Duration,
    pub graphiql: bool,
    pub commands: GraphqlCommands,
    pub schemas: HashMap<String, async_graphql::dynamic::Schema>,
    pub change_hub: super::subscribe::ChangeHub,
    pub dialect: SqlDialect,
}

pub struct GraphqlEngine {
    pub(crate) inner: Arc<EngineInner>,
}

pub struct GraphqlEngineBuilder {
    pool: GraphqlPool,
    catalog: BTreeMap<String, CatalogEntry>,
    by_table: BTreeMap<String, String>,
    permissions: BTreeMap<(String, String), RoleModelPerm>,
    roles: Option<BTreeSet<String>>,
    anonymous_role: String,
    default_limit: u64,
    max_limit: u64,
    max_depth: usize,
    max_complexity: usize,
    max_in_list: usize,
    max_bool_width: usize,
    introspection_for_anonymous: bool,
    statement_timeout: Duration,
    graphiql: bool,
    commands: GraphqlCommands,
    change_rx: Option<tokio::sync::broadcast::Receiver<ReadModelChange>>,
    pending_errors: Vec<String>,
}

impl GraphqlEngine {
    pub fn builder(pool: impl Into<GraphqlPool>) -> GraphqlEngineBuilder {
        GraphqlEngineBuilder::new(pool.into())
    }

    pub fn from_manifest(
        m: &DistributedProjectManifest,
        pool: impl Into<GraphqlPool>,
    ) -> Result<GraphqlEngineBuilder, GraphqlBuildError> {
        let mut builder = Self::builder(pool);
        for schema in &m.tables {
            if schema.kind == TableKind::ReadModel {
                builder = builder.register_schema_exposed(schema.clone())?;
            } else {
                // Operational tables stay out of the catalog for GraphQL.
            }
        }
        Ok(builder)
    }

    pub fn sdl_for_role(&self, role: &str) -> Option<String> {
        if !self.inner.roles.contains(role) && role != self.inner.anonymous_role {
            // Still allow any role that has a built schema.
        }
        self.inner.schemas.get(role).map(|s| s.sdl())
    }

    pub fn graphiql_enabled(&self) -> bool {
        self.inner.graphiql
    }

    pub async fn execute(&self, session: &Session, request: Request) -> Response {
        let role = resolve_role(session, &self.inner.anonymous_role);
        let Some(schema) = self.inner.schemas.get(&role) else {
            return Response::from_errors(vec![ServerError::new(
                format!("role `{role}` is not configured for GraphQL"),
                None,
            )]);
        };

        let request = request.data(session.clone()).data(Arc::clone(&self.inner));
        let start = std::time::Instant::now();
        let response = schema.execute(request).await;
        let status = metrics_status_for_response(&response);
        let root_field = match &response.data {
            Value::Object(map) => map.keys().next().map(|s| s.as_str()).unwrap_or("_"),
            _ => "_",
        };
        record_metrics(session, root_field, status, start.elapsed());
        let _ = role;
        response
    }

    /// Hub used by live subscriptions (tests may publish directly).
    pub fn change_hub(&self) -> &super::subscribe::ChangeHub {
        &self.inner.change_hub
    }

    /// Execute a GraphQL subscription document as a stream of responses.
    pub fn execute_stream(
        &self,
        session: &Session,
        request: Request,
    ) -> BoxStream<'static, async_graphql::Response> {
        let role = resolve_role(session, &self.inner.anonymous_role);
        let Some(schema) = self.inner.schemas.get(&role).cloned() else {
            return stream::once(async move {
                Response::from_errors(vec![ServerError::new(
                    format!("role `{role}` is not configured for GraphQL"),
                    None,
                )])
            })
            .boxed();
        };
        let request = request
            .data(session.clone())
            .data(std::sync::Arc::clone(&self.inner));
        schema.execute_stream(request).boxed()
    }
}

fn resolve_role(session: &Session, anonymous: &str) -> String {
    session
        .role()
        .filter(|r| !r.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| anonymous.to_string())
}

/// Map GraphQL response errors to coarse metric `status` labels.
///
/// Privacy: only stable class names (`ok`, `timeout`, `bad_request`,
/// `forbidden`, `internal`, `error`) — never user/tenant/SQL text.
pub(crate) fn metrics_status_for_response(response: &Response) -> &'static str {
    if !response.is_err() {
        return "ok";
    }
    for err in &response.errors {
        if let Some(ext) = &err.extensions {
            if let Some(code) = ext.get("code") {
                let code = format!("{code:?}").to_ascii_uppercase();
                if code.contains("TIMEOUT") {
                    return "timeout";
                }
                if code.contains("BAD_REQUEST") {
                    return "bad_request";
                }
                if code.contains("FORBIDDEN") {
                    return "forbidden";
                }
                if code.contains("INTERNAL") {
                    return "internal";
                }
            }
        }
        let msg = err.message.to_ascii_lowercase();
        if msg.contains("timeout") {
            return "timeout";
        }
        if msg.contains("not configured") || msg.contains("forbidden") {
            return "forbidden";
        }
    }
    "error"
}

fn record_metrics(session: &Session, root_field: &str, status: &str, duration: Duration) {
    let _ = session;
    #[cfg(feature = "metrics")]
    crate::metrics::record_graphql_request(None, root_field, status, duration);
    #[cfg(not(feature = "metrics"))]
    let _ = (root_field, status, duration);
}

impl GraphqlEngineBuilder {
    fn new(pool: GraphqlPool) -> Self {
        Self {
            pool,
            catalog: BTreeMap::new(),
            by_table: BTreeMap::new(),
            permissions: BTreeMap::new(),
            roles: None,
            anonymous_role: "anonymous".into(),
            default_limit: 100,
            max_limit: 1000,
            max_depth: 8,
            max_complexity: 500,
            max_in_list: 1000,
            max_bool_width: 256,
            introspection_for_anonymous: true,
            statement_timeout: Duration::from_secs(5),
            graphiql: false,
            commands: GraphqlCommands::new(),
            change_rx: None,
            pending_errors: Vec::new(),
        }
    }

    pub fn model<M: RelationalReadModelIncludes>(mut self, perms: ModelPermissions<M>) -> Self {
        let schema = M::schema().clone();
        if let Err(e) = self.insert_catalog(schema.clone(), true) {
            self.pending_errors.push(e.0);
            return self;
        }
        // Shadow-register one-hop relationship targets.
        for rel in &schema.relationships {
            match M::include_target_schema(&rel.field_name) {
                Ok(target) => {
                    if let Err(e) = self.insert_catalog(target.clone(), false) {
                        // Shadow insert conflict only if different schema.
                        if !e.0.contains("identical") {
                            self.pending_errors.push(e.0);
                        }
                    }
                }
                Err(err) => {
                    self.pending_errors.push(format!(
                        "include_target_schema for `{}`.`{}`: {err}",
                        schema.model_name, rel.field_name
                    ));
                }
            }
        }
        for (role, perm) in perms.entries {
            let key = (schema.model_name.clone(), role.clone());
            match self.permissions.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(RoleModelPerm { permission: perm });
                }
                Entry::Occupied(_) => {
                    self.pending_errors.push(format!(
                        "duplicate permission for model `{}` role `{role}`",
                        schema.model_name
                    ));
                }
            }
        }
        self
    }

    pub fn table_schema(mut self, schema: TableSchema) -> Self {
        if let Err(e) = self.insert_catalog(schema, false) {
            self.pending_errors.push(e.0);
        }
        self
    }

    fn register_schema_exposed(mut self, schema: TableSchema) -> Result<Self, GraphqlBuildError> {
        self.insert_catalog(schema, true)?;
        Ok(self)
    }

    fn insert_catalog(
        &mut self,
        schema: TableSchema,
        exposed: bool,
    ) -> Result<(), GraphqlBuildError> {
        schema
            .validate()
            .map_err(|e| GraphqlBuildError(e.to_string()))?;
        if let Some(existing) = self.by_table.get(&schema.table_name) {
            if existing != &schema.model_name {
                return Err(GraphqlBuildError(format!(
                    "duplicate table_name `{}` for models `{existing}` and `{}`",
                    schema.table_name, schema.model_name
                )));
            }
        }
        match self.catalog.get(&schema.model_name) {
            Some(entry) if entry.schema != schema => {
                return Err(GraphqlBuildError(format!(
                    "conflicting schemas for model `{}`",
                    schema.model_name
                )));
            }
            Some(entry) => {
                // Identical: upgrade shadow → exposed if needed.
                if exposed && !entry.exposed {
                    let mut upgraded = entry.clone();
                    upgraded.exposed = true;
                    self.catalog.insert(schema.model_name.clone(), upgraded);
                }
                return Ok(());
            }
            None => {}
        }
        self.by_table
            .insert(schema.table_name.clone(), schema.model_name.clone());
        self.catalog
            .insert(schema.model_name.clone(), CatalogEntry { schema, exposed });
        Ok(())
    }

    pub fn roles(mut self, roles: &[&str]) -> Self {
        self.roles = Some(roles.iter().map(|r| (*r).to_string()).collect());
        self
    }

    pub fn grant_all(mut self, role: &str) -> Self {
        let exposed: Vec<String> = self
            .catalog
            .iter()
            .filter(|(_, e)| e.exposed)
            .map(|(n, _)| n.clone())
            .collect();
        for model in exposed {
            let key = (model.clone(), role.to_string());
            match self.permissions.entry(key) {
                Entry::Vacant(entry) => {
                    entry.insert(RoleModelPerm {
                        permission: select().all_columns().allow_aggregations(true),
                    });
                }
                Entry::Occupied(_) => {
                    self.pending_errors.push(format!(
                        "duplicate permission for model `{model}` role `{role}`"
                    ));
                }
            }
        }
        self
    }

    pub fn permission<M: RelationalReadModelIncludes>(
        mut self,
        role: &str,
        p: SelectPermission,
    ) -> Self {
        let model = M::schema().model_name.clone();
        if !self.catalog.contains_key(&model) {
            self.pending_errors.push(format!(
                "permission for unregistered model `{model}` (role `{role}`)"
            ));
            return self;
        }
        let key = (model.clone(), role.to_string());
        match self.permissions.entry(key) {
            Entry::Vacant(entry) => {
                entry.insert(RoleModelPerm { permission: p });
            }
            Entry::Occupied(_) => {
                self.pending_errors.push(format!(
                    "duplicate permission for model `{model}` role `{role}`"
                ));
            }
        }
        self
    }

    pub fn anonymous_role(mut self, name: &str) -> Self {
        self.anonymous_role = name.to_string();
        self
    }
    pub fn default_limit(mut self, n: u64) -> Self {
        self.default_limit = n;
        self
    }
    pub fn max_limit(mut self, n: u64) -> Self {
        self.max_limit = n;
        self
    }
    pub fn max_depth(mut self, n: usize) -> Self {
        self.max_depth = n;
        self
    }
    pub fn max_complexity(mut self, n: usize) -> Self {
        self.max_complexity = n;
        self
    }
    pub fn max_in_list(mut self, n: usize) -> Self {
        self.max_in_list = n;
        self
    }
    /// Cap width of a single `_and` / `_or` list in client `where` (default 256).
    pub fn max_bool_width(mut self, n: usize) -> Self {
        self.max_bool_width = n;
        self
    }
    pub fn introspection_for_anonymous(mut self, on: bool) -> Self {
        self.introspection_for_anonymous = on;
        self
    }
    pub fn commands(mut self, c: GraphqlCommands) -> Self {
        self.commands = c;
        self
    }
    pub fn statement_timeout(mut self, d: Duration) -> Self {
        self.statement_timeout = d;
        self
    }
    pub fn graphiql(mut self, on: bool) -> Self {
        self.graphiql = on;
        self
    }
    pub fn change_stream(mut self, rx: tokio::sync::broadcast::Receiver<ReadModelChange>) -> Self {
        self.change_rx = Some(rx);
        self
    }

    pub fn build(self) -> Result<GraphqlEngine, GraphqlBuildError> {
        if !self.pending_errors.is_empty() {
            return Err(GraphqlBuildError(self.pending_errors.join("; ")));
        }

        // Validate permissions.
        let declared_roles = self.roles.clone();
        let anonymous = self.anonymous_role.clone();
        let perm_keys: Vec<_> = self.permissions.keys().cloned().collect();
        for (model, role) in &perm_keys {
            if let Some(roles) = &declared_roles {
                if !roles.contains(role) && role != &anonymous {
                    return Err(GraphqlBuildError(format!(
                        "permission for undeclared role `{role}` (model `{model}`)"
                    )));
                }
            }
            let entry = self.catalog.get(model).ok_or_else(|| {
                GraphqlBuildError(format!("permission for unknown model `{model}`"))
            })?;
            let perm = &self.permissions[&(model.clone(), role.clone())].permission;

            // Column allowlist validation.
            if !perm.all_columns {
                if let Some(cols) = &perm.columns {
                    for col in cols {
                        if !entry
                            .schema
                            .columns
                            .iter()
                            .any(|c| c.column_name == *col && !c.skipped)
                        {
                            return Err(GraphqlBuildError(format!(
                                "unknown column `{col}` in permission for `{model}` role `{role}`"
                            )));
                        }
                    }
                }
            }

            if let Some(filter) = &perm.filter {
                validate_filter(
                    filter,
                    &entry.schema,
                    &self.catalog,
                    role == &anonymous,
                    model,
                    role,
                )?;
            }
        }

        // Name grammar / collisions across exposed models.
        validate_generated_names(&self.catalog)?;

        // v1 join / by_pk paths assume a single-column primary key.
        for entry in self.catalog.values() {
            if !entry.exposed {
                continue;
            }
            let pk_n = entry.schema.primary_key.columns.len();
            if pk_n > 1 {
                return Err(GraphqlBuildError(format!(
                    "model `{}` has {pk_n}-column primary key; multi-column primary keys are not supported in GraphQL v1 (single-column PK required)",
                    entry.schema.model_name
                )));
            }
        }

        // m2m resolution for catalog relationships used in permissions/schema.
        for entry in self.catalog.values() {
            for rel in &entry.schema.relationships {
                if matches!(rel.kind, RelationshipKind::ManyToMany) {
                    if rel.through.is_none() {
                        return Err(GraphqlBuildError(format!(
                            "model `{}` relationship `{}` many-to-many must declare `through`",
                            entry.schema.model_name, rel.field_name
                        )));
                    }
                    if let (Some(target), Some(through_name)) =
                        (self.catalog.get(&rel.target_model), rel.through.as_deref())
                    {
                        if let Some(through_model) = self.by_table.get(through_name) {
                            if let Some(through) = self.catalog.get(through_model) {
                                resolve_m2m_target_foreign_key(
                                    &entry.schema,
                                    rel,
                                    &through.schema,
                                    &target.schema,
                                )
                                .map_err(|e| GraphqlBuildError(e.to_string()))?;
                            }
                        }
                    }
                }
            }
        }

        let dialect = match &self.pool {
            #[cfg(feature = "postgres")]
            GraphqlPool::Postgres(_) => SqlDialect::Postgres,
            #[cfg(feature = "sqlite")]
            GraphqlPool::Sqlite(_) => SqlDialect::Sqlite,
            #[allow(unreachable_patterns)]
            _ => {
                return Err(GraphqlBuildError(
                    "GraphqlPool has no store feature enabled".into(),
                ));
            }
        };

        let mut roles: BTreeSet<String> = self.permissions.keys().map(|(_, r)| r.clone()).collect();
        if let Some(declared) = &declared_roles {
            roles.extend(declared.iter().cloned());
        }
        roles.insert(anonymous.clone());

        // Build per-role dynamic schemas.
        let mut schemas = HashMap::new();
        for role in &roles {
            let schema = dyn_schema::build_role_schema(
                role,
                &self.catalog,
                &self.by_table,
                &self.permissions,
                &self.commands,
                self.max_depth,
                self.max_complexity,
                dialect,
                role == &anonymous && !self.introspection_for_anonymous,
            )
            .map_err(GraphqlBuildError)?;
            schemas.insert(role.clone(), schema);
        }

        let change_hub = super::subscribe::ChangeHub::new();
        if let Some(rx) = self.change_rx {
            super::subscribe::spawn_change_forwarder(change_hub.clone(), rx);
        }

        let inner = Arc::new(EngineInner {
            pool: self.pool,
            catalog: self.catalog,
            by_table: self.by_table,
            permissions: self.permissions,
            roles,
            anonymous_role: anonymous,
            default_limit: self.default_limit,
            max_limit: self.max_limit,
            max_depth: self.max_depth,
            max_complexity: self.max_complexity,
            max_in_list: self.max_in_list,
            max_bool_width: self.max_bool_width,
            introspection_for_anonymous: self.introspection_for_anonymous,
            statement_timeout: self.statement_timeout,
            graphiql: self.graphiql,
            commands: self.commands,
            schemas,
            change_hub,
            dialect,
        });

        Ok(GraphqlEngine { inner })
    }
}

/// Resolve whether GraphiQL should be enabled from environment variables.
///
/// Policy (scaffold + operators):
/// - `GRAPHIQL` if set: on unless value is `0` / `false` / `off` / `no` (case-insensitive)
/// - else: **off** when `RUST_ENV` / `ENV` / `APP_ENV` is `production` or `prod`
/// - else: **on** (local/dev default)
///
/// Pure inputs so tests do not mutate process env. See [`graphiql_enabled_from_env`].
pub fn graphiql_enabled_from_env_vars(
    graphiql: Option<&str>,
    rust_env: Option<&str>,
    env: Option<&str>,
    app_env: Option<&str>,
) -> bool {
    if let Some(v) = graphiql {
        return !matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        );
    }
    let prod = rust_env
        .or(env)
        .or(app_env)
        .unwrap_or("")
        .to_ascii_lowercase();
    !matches!(prod.as_str(), "production" | "prod")
}

/// Read process env and apply [`graphiql_enabled_from_env_vars`].
pub fn graphiql_enabled_from_env() -> bool {
    graphiql_enabled_from_env_vars(
        std::env::var("GRAPHIQL").ok().as_deref(),
        std::env::var("RUST_ENV").ok().as_deref(),
        std::env::var("ENV").ok().as_deref(),
        std::env::var("APP_ENV").ok().as_deref(),
    )
}

#[cfg(test)]
mod graphiql_env_tests {
    use super::graphiql_enabled_from_env_vars;

    #[test]
    fn production_rust_env_disables_graphiql() {
        assert!(!graphiql_enabled_from_env_vars(
            None,
            Some("production"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            None,
            Some("prod"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            None,
            Some("PRODUCTION"),
            None,
            None
        ));
    }

    #[test]
    fn non_production_enables_graphiql_by_default() {
        assert!(graphiql_enabled_from_env_vars(
            None,
            Some("development"),
            None,
            None
        ));
        assert!(graphiql_enabled_from_env_vars(None, None, None, None));
    }

    #[test]
    fn explicit_graphiql_overrides_production() {
        assert!(graphiql_enabled_from_env_vars(
            Some("1"),
            Some("production"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            Some("0"),
            Some("development"),
            None,
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            Some("false"),
            None,
            None,
            None
        ));
    }

    #[test]
    fn env_and_app_env_aliases() {
        assert!(!graphiql_enabled_from_env_vars(
            None,
            None,
            Some("production"),
            None
        ));
        assert!(!graphiql_enabled_from_env_vars(
            None,
            None,
            None,
            Some("prod")
        ));
    }
}

#[cfg(test)]
mod metrics_status_tests {
    use super::metrics_status_for_response;
    use async_graphql::{ErrorExtensionValues, Response, ServerError};

    fn response_with_code(code: &str, message: &str) -> Response {
        let mut err = ServerError::new(message, None);
        let mut ext = ErrorExtensionValues::default();
        ext.set("code", code);
        err.extensions = Some(ext);
        Response::from_errors(vec![err])
    }

    #[test]
    fn ok_when_no_errors() {
        let resp = Response::new(async_graphql::Value::Null);
        assert_eq!(metrics_status_for_response(&resp), "ok");
    }

    #[test]
    fn maps_extension_codes() {
        assert_eq!(
            metrics_status_for_response(&response_with_code("TIMEOUT", "statement timeout")),
            "timeout"
        );
        assert_eq!(
            metrics_status_for_response(&response_with_code("BAD_REQUEST", "bad request")),
            "bad_request"
        );
        assert_eq!(
            metrics_status_for_response(&response_with_code("INTERNAL", "internal error")),
            "internal"
        );
        assert_eq!(
            metrics_status_for_response(&response_with_code("FORBIDDEN", "nope")),
            "forbidden"
        );
    }

    #[test]
    fn maps_message_fallback_timeout() {
        let resp = Response::from_errors(vec![ServerError::new("statement timeout", None)]);
        assert_eq!(metrics_status_for_response(&resp), "timeout");
    }
}

fn validate_generated_names(
    catalog: &BTreeMap<String, CatalogEntry>,
) -> Result<(), GraphqlBuildError> {
    let mut names: BTreeSet<String> = reserved_type_names().map(str::to_string).collect();
    for entry in catalog.values().filter(|e| e.exposed) {
        let schema = &entry.schema;
        for name in [
            object_type_name(schema).to_string(),
            root_list_field(schema).to_string(),
            by_pk_field(schema),
            format!("{}_bool_exp", schema.table_name),
            format!("{}_order_by", schema.table_name),
            format!("{}_aggregate", schema.table_name),
        ] {
            if !is_valid_graphql_name(&name) {
                return Err(GraphqlBuildError(format!(
                    "generated name `{name}` is not a valid GraphQL name"
                )));
            }
            if !names.insert(name.clone()) {
                return Err(GraphqlBuildError(format!(
                    "generated name `{name}` collides with another type or field"
                )));
            }
        }
    }
    Ok(())
}

fn validate_filter(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    is_anonymous: bool,
    model: &str,
    role: &str,
) -> Result<(), GraphqlBuildError> {
    if is_anonymous {
        let mut claims = Vec::new();
        filter.visit_claims(|c| claims.push(c.to_string()));
        if !claims.is_empty() {
            return Err(GraphqlBuildError(format!(
                "claim() is not allowed in anonymous role filters (model `{model}`, claims: {})",
                claims.join(", ")
            )));
        }
    }

    filter.visit_columns(|col| {
        let _ = col;
    });
    // Re-walk for proper error returns.
    validate_filter_inner(filter, schema, catalog, model, role)
}

fn validate_filter_inner(
    filter: &FilterExpr,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    model: &str,
    role: &str,
) -> Result<(), GraphqlBuildError> {
    match filter {
        FilterExpr::And(xs) | FilterExpr::Or(xs) => {
            for x in xs {
                validate_filter_inner(x, schema, catalog, model, role)?;
            }
        }
        FilterExpr::Not(x) => validate_filter_inner(x, schema, catalog, model, role)?,
        FilterExpr::Cmp { column, op, rhs } => {
            let col = schema
                .columns
                .iter()
                .find(|c| c.column_name == *column)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "unknown column `{column}` in filter for `{model}` role `{role}`"
                    ))
                })?;
            if matches!(col.column_type, ColumnType::Json) && matches!(rhs, Operand::Claim(_)) {
                return Err(GraphqlBuildError(format!(
                    "claims cannot compare to Json columns (`{column}` on `{model}`)"
                )));
            }
            let _ = op;
        }
        FilterExpr::In { column, .. } | FilterExpr::IsNull { column, .. } => {
            if !schema.columns.iter().any(|c| c.column_name == *column) {
                return Err(GraphqlBuildError(format!(
                    "unknown column `{column}` in filter for `{model}` role `{role}`"
                )));
            }
        }
        FilterExpr::Rel { field, predicate } => {
            let rel = schema
                .relationships
                .iter()
                .find(|r| r.field_name == *field)
                .ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "rel(`{field}`) is not a relationship on model `{model}`"
                    ))
                })?;
            let target = catalog.get(&rel.target_model).ok_or_else(|| {
                GraphqlBuildError(format!(
                    "rel(`{field}`) target `{}` is not in the catalog (model `{model}`)",
                    rel.target_model
                ))
            })?;
            if matches!(rel.kind, RelationshipKind::ManyToMany) {
                let through = rel.through.as_deref().ok_or_else(|| {
                    GraphqlBuildError(format!(
                        "rel(`{field}`) many-to-many missing through on `{model}`"
                    ))
                })?;
                let through_model = catalog
                    .values()
                    .find(|e| e.schema.table_name == through)
                    .ok_or_else(|| {
                        GraphqlBuildError(format!(
                            "rel(`{field}`) through table `{through}` not in catalog"
                        ))
                    })?;
                let _ = through_model;
            }
            validate_filter_inner(predicate, &target.schema, catalog, &rel.target_model, role)?;
        }
    }
    Ok(())
}

/// Execute a compiled plan against the engine pool (used by root resolvers).
pub(crate) async fn execute_plan(inner: &EngineInner, plan: &SqlPlan) -> Result<Value, String> {
    execute::execute_sql(inner, plan).await
}

/// Public helper for tests: compile + naming surface.
#[allow(dead_code)]
pub fn core_sdl_for_catalog(tables: &[TableSchema]) -> Result<String, String> {
    graphql_sdl_for_tables_with_options(
        tables,
        &SdlOptions {
            aggregates: true,
            jsonb_operators: false,
            subscriptions: true,
        },
    )
}
