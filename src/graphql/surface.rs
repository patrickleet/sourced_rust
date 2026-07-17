//! Shared GraphQL **surface IR** — single source of truth for the query/subscription
//! type system a catalog (and optionally a role) can see.
//!
//! SDL emission and (over time) runtime schema construction consume this IR so
//! dialect-honest comparison ops, roots, and column grants cannot diverge.
//!
//! Core types compile without the `graphql` feature so `dctl schema --format graphql`
//! can share the same IR path.

use std::collections::{BTreeMap, BTreeSet};

use crate::table::{
    resolve_m2m_target_foreign_key, RelationshipKind, TableColumn, TableSchema,
};

use super::naming::{
    by_pk_field, comparison_exp_name, comparison_op_fields, include_postgres_json_comparison_ops,
    is_valid_graphql_name, object_type_name, root_list_field, scalar_type_name, CUSTOM_SCALARS,
};

/// Dialect gate for comparison operators (JSON ops only on Postgres).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceDialect {
    Sqlite,
    Postgres,
}

impl SurfaceDialect {
    pub fn is_postgres(self) -> bool {
        matches!(self, Self::Postgres)
    }
}

/// Options for building a surface from a table catalog.
#[derive(Clone, Debug)]
pub struct SurfaceOptions {
    pub dialect: SurfaceDialect,
    pub aggregates: bool,
    pub subscriptions: bool,
}

impl SurfaceOptions {
    pub fn sqlite() -> Self {
        Self {
            dialect: SurfaceDialect::Sqlite,
            aggregates: true,
            subscriptions: true,
        }
    }

    pub fn postgres() -> Self {
        Self {
            dialect: SurfaceDialect::Postgres,
            aggregates: true,
            subscriptions: true,
        }
    }
}

/// Kind of a GraphQL root field on Query / Subscription.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    List,
    ByPk,
    Aggregate,
}

/// One Query or Subscription root field inventory entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootField {
    pub name: String,
    pub kind: RootKind,
    /// GraphQL object type name (`model_name`).
    pub object: String,
    /// Model name in the catalog (`schema.model_name`).
    pub model_name: String,
}

/// Column field on an object type (after skips / role filter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnField {
    pub name: String,
    pub scalar: String,
    pub nullable: bool,
}

/// Relationship field inventory (target must be on the surface).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelField {
    pub name: String,
    pub target_model: String,
    pub target_object: String,
    pub kind: RelationshipKind,
    pub list: bool,
}

/// One exposed read-model on the surface.
#[derive(Clone, Debug)]
pub struct SurfaceModel {
    pub model_name: String,
    pub table_name: String,
    pub object_name: String,
    pub columns: Vec<ColumnField>,
    pub relationships: Vec<RelField>,
    pub primary_key: Vec<String>,
    /// Filtered schema clone (columns limited for role surfaces).
    pub(crate) schema: TableSchema,
}

/// Intermediate surface IR.
#[derive(Clone, Debug)]
pub struct Surface {
    pub dialect: SurfaceDialect,
    pub aggregates: bool,
    pub subscriptions: bool,
    /// Keyed by `model_name`.
    pub models: BTreeMap<String, SurfaceModel>,
    pub query_fields: Vec<RootField>,
    pub subscription_fields: Vec<RootField>,
    /// GraphQL comparison input name → operator field names (from `naming` only).
    pub comparison_ops: BTreeMap<String, Vec<String>>,
}

impl Surface {
    /// Read-model schemas in stable model_name order (for SDL / adapters).
    pub fn schemas(&self) -> Vec<TableSchema> {
        self.models
            .values()
            .map(|m| m.schema.clone())
            .collect()
    }

    /// Inventory of query root field names (sorted).
    pub fn query_root_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.query_fields.iter().map(|f| f.name.as_str()).collect();
        names.sort();
        names
    }

    /// Comparison operator fields for a scalar, empty if scalar unused.
    pub fn comparison_ops_for_scalar(&self, scalar: &str) -> Vec<&str> {
        let name = comparison_exp_name(scalar);
        self.comparison_ops
            .get(&name)
            .map(|ops| ops.iter().map(String::as_str).collect())
            .unwrap_or_default()
    }
}

/// Build the full (unscoped) surface from a table catalog.
pub fn build_surface(
    tables: &[TableSchema],
    options: &SurfaceOptions,
) -> Result<Surface, String> {
    let read_models: Vec<&TableSchema> = tables
        .iter()
        .filter(|t| t.kind.is_read_model())
        .collect();

    for schema in &read_models {
        schema
            .validate()
            .map_err(|e| format!("schema `{}` invalid: {e}", schema.model_name))?;
    }

    let by_model: BTreeMap<&str, &TableSchema> = read_models
        .iter()
        .map(|t| (t.model_name.as_str(), *t))
        .collect();
    let by_table: BTreeMap<&str, &TableSchema> = read_models
        .iter()
        .map(|t| (t.table_name.as_str(), *t))
        .collect();

    let postgres_json = include_postgres_json_comparison_ops(options.dialect.is_postgres());
    let mut used_scalars: BTreeSet<String> = BTreeSet::new();
    let mut models: BTreeMap<String, SurfaceModel> = BTreeMap::new();

    for schema in &read_models {
        let object_name = object_type_name(schema).to_string();
        if !is_valid_graphql_name(&object_name) {
            return Err(format!("object type `{object_name}` is not a valid GraphQL name"));
        }
        if !is_valid_graphql_name(root_list_field(schema)) {
            return Err(format!(
                "root field `{}` is not a valid GraphQL name",
                root_list_field(schema)
            ));
        }

        let mut columns = Vec::new();
        for col in visible_columns(schema) {
            if !is_valid_graphql_name(&col.column_name) {
                return Err(format!(
                    "model `{}` column `{}` is not a valid GraphQL name",
                    schema.model_name, col.column_name
                ));
            }
            let Some(scalar) = scalar_type_name(&col.column_type) else {
                return Err(format!(
                    "model `{}` column `{}` has unsupported type",
                    schema.model_name, col.column_name
                ));
            };
            used_scalars.insert(scalar.to_string());
            columns.push(ColumnField {
                name: col.column_name.clone(),
                scalar: scalar.to_string(),
                nullable: col.nullable,
            });
        }

        let mut relationships = Vec::new();
        for rel in &schema.relationships {
            if !is_valid_graphql_name(&rel.field_name) {
                return Err(format!(
                    "model `{}` relationship `{}` is not a valid GraphQL name",
                    schema.model_name, rel.field_name
                ));
            }
            if !relationship_emitted(schema, rel, &by_model, &by_table) {
                continue;
            }
            let target = by_model
                .get(rel.target_model.as_str())
                .expect("relationship_emitted");
            let list = matches!(
                rel.kind,
                RelationshipKind::HasMany | RelationshipKind::ManyToMany
            );
            relationships.push(RelField {
                name: rel.field_name.clone(),
                target_model: rel.target_model.clone(),
                target_object: object_type_name(target).to_string(),
                kind: rel.kind.clone(),
                list,
            });
        }

        models.insert(
            schema.model_name.clone(),
            SurfaceModel {
                model_name: schema.model_name.clone(),
                table_name: schema.table_name.clone(),
                object_name,
                columns,
                relationships,
                primary_key: schema.primary_key.columns.clone(),
                schema: (*schema).clone(),
            },
        );
    }

    // Drop relationships whose target was not included (defensive).
    let model_keys: BTreeSet<String> = models.keys().cloned().collect();
    for model in models.values_mut() {
        model
            .relationships
            .retain(|r| model_keys.contains(&r.target_model));
    }

    let mut comparison_ops = BTreeMap::new();
    for scalar in &used_scalars {
        let ops = comparison_op_fields(scalar, postgres_json);
        comparison_ops.insert(
            comparison_exp_name(scalar),
            ops.into_iter().map(str::to_string).collect(),
        );
    }
    // Always reserve custom scalar names for naming collisions checks downstream.
    let _ = CUSTOM_SCALARS;

    let (query_fields, subscription_fields) =
        root_fields_for_models(&models, options.aggregates, options.subscriptions);

    Ok(Surface {
        dialect: options.dialect,
        aggregates: options.aggregates,
        subscriptions: options.subscriptions,
        models,
        query_fields,
        subscription_fields,
        comparison_ops,
    })
}

/// Role grant used by [`surface_for_role`] (feature-free; maps from `ReadPermission`
/// when the `graphql` feature is enabled).
#[derive(Clone, Debug)]
pub struct RoleGrant {
    pub all_columns: bool,
    pub columns: BTreeSet<String>,
    pub aggregations: bool,
}

impl RoleGrant {
    pub fn all_columns() -> Self {
        Self {
            all_columns: true,
            columns: BTreeSet::new(),
            aggregations: false,
        }
    }

    pub fn columns<I: IntoIterator<Item = impl Into<String>>>(cols: I) -> Self {
        Self {
            all_columns: false,
            columns: cols.into_iter().map(Into::into).collect(),
            aggregations: false,
        }
    }

    pub fn with_aggregations(mut self) -> Self {
        self.aggregations = true;
        self
    }

    pub fn allows_column(&self, name: &str) -> bool {
        self.all_columns || self.columns.contains(name)
    }
}

/// Apply role grants: drop ungranted models and columns (and relationships to
/// dropped models). Aggregate roots omitted when `aggregations` is false.
///
/// `grants`: map of model_name → grant for this role. Missing model = not granted.
pub fn surface_for_role(
    surface: &Surface,
    role: &str,
    grants: &BTreeMap<String, RoleGrant>,
) -> Surface {
    let _ = role;
    let mut models: BTreeMap<String, SurfaceModel> = BTreeMap::new();

    for (model_name, model) in &surface.models {
        let Some(grant) = grants.get(model_name) else {
            continue;
        };

        let allowed_cols: BTreeSet<String> = model
            .columns
            .iter()
            .filter(|c| grant.allows_column(&c.name))
            .map(|c| c.name.clone())
            .collect();

        let columns: Vec<ColumnField> = model
            .columns
            .iter()
            .filter(|c| allowed_cols.contains(&c.name))
            .cloned()
            .collect();

        let mut schema = model.schema.clone();
        for col in &mut schema.columns {
            if !col.skipped && !allowed_cols.contains(&col.column_name) {
                col.skipped = true;
            }
        }

        models.insert(
            model_name.clone(),
            SurfaceModel {
                model_name: model.model_name.clone(),
                table_name: model.table_name.clone(),
                object_name: model.object_name.clone(),
                columns,
                relationships: model.relationships.clone(),
                primary_key: model.primary_key.clone(),
                schema,
            },
        );
    }

    // Relationships only if target model remains granted (collect keys first).
    let model_keys: BTreeSet<String> = models.keys().cloned().collect();
    for model in models.values_mut() {
        model
            .relationships
            .retain(|r| model_keys.contains(&r.target_model));
        let rel_names: BTreeSet<String> = model.relationships.iter().map(|r| r.name.clone()).collect();
        model.schema.relationships.retain(|r| {
            model_keys.contains(&r.target_model) && rel_names.contains(&r.field_name)
        });
    }

    let mut query_fields = Vec::new();
    let mut subscription_fields = Vec::new();
    for model in models.values() {
        let grant = grants.get(&model.model_name);
        let allow_agg = surface.aggregates && grant.is_some_and(|g| g.aggregations);
        let list = root_list_field(&model.schema).to_string();
        let by_pk = by_pk_field(&model.schema);
        query_fields.push(RootField {
            name: list.clone(),
            kind: RootKind::List,
            object: model.object_name.clone(),
            model_name: model.model_name.clone(),
        });
        query_fields.push(RootField {
            name: by_pk.clone(),
            kind: RootKind::ByPk,
            object: model.object_name.clone(),
            model_name: model.model_name.clone(),
        });
        if allow_agg {
            query_fields.push(RootField {
                name: format!("{}_aggregate", model.table_name),
                kind: RootKind::Aggregate,
                object: model.object_name.clone(),
                model_name: model.model_name.clone(),
            });
        }
        if surface.subscriptions {
            subscription_fields.push(RootField {
                name: list,
                kind: RootKind::List,
                object: model.object_name.clone(),
                model_name: model.model_name.clone(),
            });
            subscription_fields.push(RootField {
                name: by_pk,
                kind: RootKind::ByPk,
                object: model.object_name.clone(),
                model_name: model.model_name.clone(),
            });
        }
    }
    query_fields.sort_by(|a, b| a.name.cmp(&b.name));
    subscription_fields.sort_by(|a, b| a.name.cmp(&b.name));

    let postgres_json = include_postgres_json_comparison_ops(surface.dialect.is_postgres());
    let mut used_scalars: BTreeSet<String> = BTreeSet::new();
    for m in models.values() {
        for c in &m.columns {
            used_scalars.insert(c.scalar.clone());
        }
    }
    let mut comparison_ops = BTreeMap::new();
    for scalar in &used_scalars {
        let ops = comparison_op_fields(scalar, postgres_json);
        comparison_ops.insert(
            comparison_exp_name(scalar),
            ops.into_iter().map(str::to_string).collect(),
        );
    }

    let aggregates = query_fields.iter().any(|f| f.kind == RootKind::Aggregate);

    Surface {
        dialect: surface.dialect,
        aggregates,
        subscriptions: surface.subscriptions,
        models,
        query_fields,
        subscription_fields,
        comparison_ops,
    }
}

fn root_fields_for_models(
    models: &BTreeMap<String, SurfaceModel>,
    aggregates: bool,
    subscriptions: bool,
) -> (Vec<RootField>, Vec<RootField>) {
    let mut query_fields = Vec::new();
    let mut subscription_fields = Vec::new();
    for model in models.values() {
        let list = root_list_field(&model.schema).to_string();
        let by_pk = by_pk_field(&model.schema);
        query_fields.push(RootField {
            name: list.clone(),
            kind: RootKind::List,
            object: model.object_name.clone(),
            model_name: model.model_name.clone(),
        });
        query_fields.push(RootField {
            name: by_pk.clone(),
            kind: RootKind::ByPk,
            object: model.object_name.clone(),
            model_name: model.model_name.clone(),
        });
        if aggregates {
            query_fields.push(RootField {
                name: format!("{}_aggregate", model.table_name),
                kind: RootKind::Aggregate,
                object: model.object_name.clone(),
                model_name: model.model_name.clone(),
            });
        }
        if subscriptions {
            subscription_fields.push(RootField {
                name: list,
                kind: RootKind::List,
                object: model.object_name.clone(),
                model_name: model.model_name.clone(),
            });
            subscription_fields.push(RootField {
                name: by_pk,
                kind: RootKind::ByPk,
                object: model.object_name.clone(),
                model_name: model.model_name.clone(),
            });
        }
    }
    query_fields.sort_by(|a, b| a.name.cmp(&b.name));
    subscription_fields.sort_by(|a, b| a.name.cmp(&b.name));
    (query_fields, subscription_fields)
}

fn visible_columns(schema: &TableSchema) -> impl Iterator<Item = &TableColumn> {
    schema.columns.iter().filter(|c| !c.skipped)
}

fn relationship_emitted(
    schema: &TableSchema,
    rel: &crate::table::RelationshipDef,
    by_model: &BTreeMap<&str, &TableSchema>,
    by_table: &BTreeMap<&str, &TableSchema>,
) -> bool {
    let Some(target) = by_model.get(rel.target_model.as_str()) else {
        return false;
    };
    match rel.kind {
        RelationshipKind::HasMany | RelationshipKind::BelongsTo => true,
        RelationshipKind::ManyToMany => {
            let Some(through_name) = rel.through.as_deref() else {
                return false;
            };
            if let Some(through) = by_table.get(through_name) {
                resolve_m2m_target_foreign_key(schema, rel, through, target).is_ok()
            } else {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{ColumnType, PrimaryKey, RelationshipDef, TableColumn, TableKind};

    fn orders() -> TableSchema {
        TableSchema {
            model_name: "OrderView".into(),
            table_name: "orders".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("order_id", "order_id", ColumnType::Text)
                },
                TableColumn::new("customer_id", "customer_id", ColumnType::Text),
                TableColumn::new("status", "status", ColumnType::Text),
                TableColumn {
                    jsonb: true,
                    ..TableColumn::new("meta", "meta", ColumnType::Json)
                },
            ],
            primary_key: PrimaryKey::new(["order_id"]),
            version_column: Some("_sourced_version".into()),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        }
    }

    fn operational() -> TableSchema {
        TableSchema {
            model_name: "Outbox".into(),
            table_name: "outbox".into(),
            columns: vec![TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            }],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::Operational,
        }
    }

    #[test]
    fn build_surface_skips_operational_and_lists_roots() {
        let surface = build_surface(
            &[orders(), operational()],
            &SurfaceOptions::sqlite(),
        )
        .expect("surface");
        assert!(surface.models.contains_key("OrderView"));
        assert!(!surface.models.contains_key("Outbox"));
        let roots = surface.query_root_names();
        assert!(roots.contains(&"orders"));
        assert!(roots.contains(&"orders_by_pk"));
        assert!(roots.contains(&"orders_aggregate"));
    }

    #[test]
    fn sqlite_surface_omits_pg_json_comparison_ops() {
        let surface = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let ops = surface.comparison_ops_for_scalar("JSON");
        assert!(ops.contains(&"_eq"));
        for forbidden in ["_contains", "_contained_in", "_has_key"] {
            assert!(
                !ops.contains(&forbidden),
                "SQLite must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn postgres_surface_includes_pg_json_comparison_ops() {
        let surface = build_surface(&[orders()], &SurfaceOptions::postgres()).unwrap();
        let ops = surface.comparison_ops_for_scalar("JSON");
        for required in ["_contains", "_contained_in", "_has_key"] {
            assert!(ops.contains(&required), "Postgres missing {required}");
        }
    }

    #[test]
    fn surface_for_role_drops_ungranted_columns_and_models() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let mut grants = BTreeMap::new();
        grants.insert(
            "OrderView".to_string(),
            RoleGrant::columns(["order_id", "status"]),
        );
        let role_surface = surface_for_role(&full, "user", &grants);
        let model = role_surface.models.get("OrderView").expect("granted");
        let col_names: Vec<_> = model.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(col_names, vec!["order_id", "status"]);
        assert!(!col_names.contains(&"customer_id"));
        assert!(!col_names.contains(&"meta"));

        let empty = surface_for_role(&full, "anon", &BTreeMap::new());
        assert!(empty.models.is_empty());
        assert!(empty.query_fields.is_empty());
    }

    #[test]
    fn surface_for_role_omits_aggregate_without_grant() {
        let full = build_surface(&[orders()], &SurfaceOptions::sqlite()).unwrap();
        let mut grants = BTreeMap::new();
        grants.insert("OrderView".to_string(), RoleGrant::all_columns());
        let role_surface = surface_for_role(&full, "user", &grants);
        let names = role_surface.query_root_names();
        assert!(names.contains(&"orders"));
        assert!(!names.contains(&"orders_aggregate"));

        let mut admin = BTreeMap::new();
        admin.insert(
            "OrderView".to_string(),
            RoleGrant::all_columns().with_aggregations(),
        );
        let admin_surface = surface_for_role(&full, "admin", &admin);
        assert!(admin_surface
            .query_root_names()
            .contains(&"orders_aggregate"));
    }

    #[test]
    fn relationship_only_when_target_on_surface() {
        let parent = TableSchema {
            model_name: "ParentView".into(),
            table_name: "parents".into(),
            columns: vec![TableColumn {
                primary_key: true,
                ..TableColumn::new("id", "id", ColumnType::Text)
            }],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: vec![RelationshipDef {
                field_name: "children".into(),
                kind: RelationshipKind::HasMany,
                target_model: "ChildView".into(),
                foreign_key: Some("parent_id".into()),
                through: None,
                target_foreign_key: None,
            }],
            kind: TableKind::ReadModel,
        };
        let child = TableSchema {
            model_name: "ChildView".into(),
            table_name: "children".into(),
            columns: vec![
                TableColumn {
                    primary_key: true,
                    ..TableColumn::new("id", "id", ColumnType::Text)
                },
                TableColumn::new("parent_id", "parent_id", ColumnType::Text),
            ],
            primary_key: PrimaryKey::new(["id"]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships: Vec::new(),
            kind: TableKind::ReadModel,
        };
        let both = build_surface(&[parent.clone(), child], &SurfaceOptions::sqlite()).unwrap();
        assert!(both
            .models
            .get("ParentView")
            .unwrap()
            .relationships
            .iter()
            .any(|r| r.name == "children"));

        let parent_only = build_surface(&[parent], &SurfaceOptions::sqlite()).unwrap();
        assert!(parent_only
            .models
            .get("ParentView")
            .unwrap()
            .relationships
            .is_empty());
    }

    /// Production path: build_surface → surface_for_role → SDL (gap A10).
    #[test]
    fn role_sdl_production_path_omits_ungranted_columns() {
        use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

        let mut grants = BTreeMap::new();
        grants.insert(
            "OrderView".to_string(),
            RoleGrant::columns(["order_id", "status"]),
        );
        let sdl = graphql_sdl_for_role(
            &[orders()],
            &SdlOptions::sqlite(),
            "user",
            &grants,
        )
        .expect("role sdl");

        // Granted
        assert!(
            sdl.contains("order_id") && sdl.contains("status"),
            "expected granted columns in SDL: {sdl}"
        );
        // Ungranted column fields must not appear on the object type body.
        // (meta / customer_id were not granted)
        assert!(
            !sdl.contains("customer_id"),
            "ungranted customer_id leaked into role SDL: {sdl}"
        );
        assert!(
            !sdl.contains("meta"),
            "ungranted meta leaked into role SDL: {sdl}"
        );
        // SQLite: no PG JSON ops even if JSON columns were granted
        for forbidden in ["_contains", "_contained_in", "_has_key"] {
            assert!(
                !sdl.contains(forbidden),
                "SQLite role SDL must not expose {forbidden}"
            );
        }
    }

    #[test]
    fn role_sdl_empty_grants_has_no_query_roots() {
        use super::super::sdl::{graphql_sdl_for_role, SdlOptions};

        let sdl = graphql_sdl_for_role(
            &[orders()],
            &SdlOptions::sqlite(),
            "anon",
            &BTreeMap::new(),
        )
        .expect("empty role sdl");
        // No list roots for orders when model ungranted
        assert!(
            !sdl.contains("orders(") && !sdl.contains("orders:"),
            "empty grants should not expose orders roots: {sdl}"
        );
    }
}
