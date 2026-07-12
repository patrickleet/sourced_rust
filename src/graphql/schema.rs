//! Per-role dynamic schema construction (async-graphql).
#![allow(clippy::items_after_test_module, clippy::too_many_arguments)]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_graphql::dynamic::{
    Field, FieldFuture, InputObject, InputValue, Object, Scalar, Schema, SchemaError, TypeRef,
};
use async_graphql::Value;

use crate::microsvc::Session;
use crate::table::{RelationshipKind, TableSchema};

use super::commands::{CommandInput, CommandOutput, GraphqlCommands};
use super::compile::{self, RootKind, SqlDialect};
use super::engine::{CatalogEntry, EngineInner, RoleModelPerm};
use super::naming::{
    bool_exp_name, by_pk_field, comparison_exp_name, include_postgres_json_comparison_ops,
    object_type_name, order_by_name, root_list_field, scalar_type_name, CUSTOM_SCALARS,
};
use super::permissions::SelectPermission;

pub fn build_role_schema(
    role: &str,
    catalog: &BTreeMap<String, CatalogEntry>,
    by_table: &BTreeMap<String, String>,
    permissions: &BTreeMap<(String, String), RoleModelPerm>,
    commands: &GraphqlCommands,
    max_depth: usize,
    max_complexity: usize,
    dialect: SqlDialect,
    disable_introspection: bool,
) -> Result<Schema, String> {
    let _ = by_table;

    // Collect models granted to this role.
    let granted: Vec<(&str, &TableSchema, &SelectPermission)> = permissions
        .iter()
        .filter(|((_, r), _)| r == role)
        .filter_map(|((model, _), perm)| {
            let entry = catalog.get(model)?;
            if !entry.exposed {
                return None;
            }
            Some((model.as_str(), &entry.schema, &perm.permission))
        })
        .collect();

    let mut query = Object::new("Query");
    let mut registered_objects: BTreeMap<String, Object> = BTreeMap::new();
    let mut registered_inputs: BTreeMap<String, InputObject> = BTreeMap::new();
    let mut scalars_needed = std::collections::BTreeSet::new();

    for scalar in CUSTOM_SCALARS {
        scalars_needed.insert(*scalar);
    }

    // order_by enum as string scalar alternative — use InputObject with enum-like strings
    // via TypeRef::named("order_by"). We'll register a scalar for simplicity in dynamic mode
    // and accept enum values as strings. For fuller fidelity use Enum type.
    let order_by_enum = async_graphql::dynamic::Enum::new("order_by")
        .item("asc")
        .item("asc_nulls_first")
        .item("asc_nulls_last")
        .item("desc")
        .item("desc_nulls_first")
        .item("desc_nulls_last");

    for (model_name, schema, perm) in &granted {
        let model_name = (*model_name).to_string();
        let table = root_list_field(schema).to_string();
        let by_pk = by_pk_field(schema);
        let obj_name = object_type_name(schema).to_string();
        let bool_exp = bool_exp_name(schema);
        let order_by = order_by_name(schema);

        ensure_object_type(
            &mut registered_objects,
            schema,
            catalog,
            permissions,
            role,
            perm,
            &mut scalars_needed,
        );
        ensure_bool_exp(
            &mut registered_inputs,
            schema,
            catalog,
            permissions,
            role,
            perm,
            &mut scalars_needed,
        );
        ensure_order_by_input(&mut registered_inputs, schema, perm);

        // List root
        let model_for_resolver = model_name.clone();
        let list_field = Field::new(
            table.clone(),
            TypeRef::named_nn_list_nn(obj_name.clone()),
            move |ctx| {
                let model = model_for_resolver.clone();
                FieldFuture::new(async move { resolve_root(&ctx, &model, RootKind::List).await })
            },
        )
        .argument(InputValue::new("where", TypeRef::named(bool_exp.clone())))
        .argument(InputValue::new(
            "order_by",
            TypeRef::named_nn_list(order_by.clone()),
        ))
        .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
        .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)));
        query = query.field(list_field);

        // by_pk root
        let model_for_pk = model_name.clone();
        let mut pk_field = Field::new(by_pk, TypeRef::named(obj_name.clone()), move |ctx| {
            let model = model_for_pk.clone();
            FieldFuture::new(async move { resolve_root(&ctx, &model, RootKind::ByPk).await })
        });
        for pk in &schema.primary_key.columns {
            if let Some(col) = schema.columns.iter().find(|c| c.column_name == *pk) {
                if let Some(scalar) = scalar_type_name(&col.column_type) {
                    pk_field =
                        pk_field.argument(InputValue::new(pk.as_str(), TypeRef::named_nn(scalar)));
                }
            }
        }
        query = query.field(pk_field);

        // Aggregate root when allowed
        if perm.allow_aggregations {
            let agg_name = format!("{}_aggregate", schema.table_name);
            let agg_type = format!("{}_aggregate", schema.table_name);
            ensure_aggregate_type(&mut registered_objects, schema);
            let model_for_agg = model_name.clone();
            let agg_field = Field::new(agg_name, TypeRef::named(agg_type), move |ctx| {
                let model = model_for_agg.clone();
                FieldFuture::new(
                    async move { resolve_root(&ctx, &model, RootKind::Aggregate).await },
                )
            })
            .argument(InputValue::new("where", TypeRef::named(bool_exp)));
            query = query.field(agg_field);
        }
    }

    // Comparison input types for used scalars
    for scalar in &scalars_needed {
        if matches!(
            *scalar,
            "String" | "Boolean" | "BigInt" | "Float" | "JSON" | "Timestamptz" | "Bytea"
        ) {
            let scalar_name = *scalar;
            let name = comparison_exp_name(scalar_name);
            registered_inputs.entry(name.clone()).or_insert_with(|| {
                let mut input = InputObject::new(name);
                for op in ["_eq", "_neq", "_gt", "_gte", "_lt", "_lte"] {
                    input = input.field(InputValue::new(op, TypeRef::named(scalar_name)));
                }
                // Optional lists: [T!] (not [T!]!)
                input = input.field(InputValue::new("_in", TypeRef::named_nn_list(scalar_name)));
                input = input.field(InputValue::new("_nin", TypeRef::named_nn_list(scalar_name)));
                input = input.field(InputValue::new(
                    "_is_null",
                    TypeRef::named(TypeRef::BOOLEAN),
                ));
                if scalar_name == "String" {
                    input = input.field(InputValue::new("_like", TypeRef::named("String")));
                    input = input.field(InputValue::new("_ilike", TypeRef::named("String")));
                }
                // Dialect-honest: PG jsonb ops only when engine dialect is Postgres.
                let pg_json = include_postgres_json_comparison_ops(matches!(
                    dialect,
                    SqlDialect::Postgres
                ));
                if scalar_name == "JSON" && pg_json {
                    input = input.field(InputValue::new("_contains", TypeRef::named("JSON")));
                    input = input.field(InputValue::new("_contained_in", TypeRef::named("JSON")));
                    input = input.field(InputValue::new("_has_key", TypeRef::named("String")));
                }
                input
            });
        }
    }

    // Empty-role Query must still define ≥1 field (async-graphql requirement).
    // Spec: empty role → FORBIDDEN fixed response on any selection.
    if granted.is_empty() {
        query = query.field(Field::new(
            "_empty",
            TypeRef::named_nn(TypeRef::BOOLEAN),
            |_| {
                FieldFuture::new(async {
                    Err::<Option<Value>, _>(async_graphql::Error::new(
                        "FORBIDDEN: role has no GraphQL grants",
                    ))
                })
            },
        ));
    }

    // Mutation root from commands
    let mut mutation: Option<Object> = None;
    let role_commands: Vec<_> = commands
        .commands
        .iter()
        .filter(|(_, c)| c.roles.is_empty() || c.roles.iter().any(|r| r == role))
        .collect();
    if !role_commands.is_empty() {
        let mut mut_obj = Object::new("Mutation");
        for (cmd_name, cmd) in role_commands {
            let field_name = cmd.resolved_field_name(cmd_name);
            let output_type = match &cmd.output {
                CommandOutput::Json => "JSON",
                CommandOutput::Typed(t) => t.name.as_str(),
            };
            let cmd_name = cmd_name.clone();
            let mut field = Field::new(field_name, TypeRef::named_nn(output_type), move |ctx| {
                let cmd_name = cmd_name.clone();
                FieldFuture::new(async move { resolve_command(&ctx, &cmd_name).await })
            });
            match &cmd.input {
                CommandInput::None => {}
                CommandInput::Json => {
                    field = field.argument(InputValue::new("input", TypeRef::named_nn("JSON")));
                }
                CommandInput::Typed(tdef) => {
                    // Register nested input type if needed
                    ensure_command_input(&mut registered_inputs, tdef);
                    field = field.argument(InputValue::new(
                        "input",
                        TypeRef::named_nn(tdef.name.as_str()),
                    ));
                }
            }
            mut_obj = mut_obj.field(field);
        }
        mutation = Some(mut_obj);
    }

    // Subscription root: live queries refreshed off ChangeHub (commit-path).
    use async_graphql::dynamic::{Subscription, SubscriptionField, SubscriptionFieldFuture};
    let mut subscription = Subscription::new("Subscription");
    let mut has_subscription = false;
    for (model_name, schema, _perm) in &granted {
        let table = root_list_field(schema).to_string();
        let obj_name = object_type_name(schema).to_string();
        let bool_exp = bool_exp_name(schema);
        let order_by = order_by_name(schema);
        let model_for_sub = (*model_name).to_string();
        let field =
            SubscriptionField::new(table, TypeRef::named_nn_list_nn(obj_name), move |ctx| {
                let model = model_for_sub.clone();
                // Extract owned data before the async block (stream is 'static).
                let inner = ctx.data_opt::<Arc<EngineInner>>().cloned();
                let session = ctx
                    .data_opt::<Session>()
                    .cloned()
                    .unwrap_or_else(Session::new);
                let selection = compile::selection_from_field(ctx.field());
                SubscriptionFieldFuture::new(async move {
                    let inner = inner.ok_or_else(|| {
                        async_graphql::Error::new("GraphqlEngine not in request data")
                    })?;
                    let role = session
                        .role()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| inner.anonymous_role.clone());
                    let stream =
                        super::subscribe::live_query_stream(inner, session, role, model, selection)
                            .await
                            .map_err(async_graphql::Error::new)?;
                    Ok(stream)
                })
            })
            .argument(InputValue::new("where", TypeRef::named(bool_exp)))
            .argument(InputValue::new(
                "order_by",
                TypeRef::named_nn_list(order_by),
            ))
            .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
            .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)));
        subscription = subscription.field(field);
        has_subscription = true;
    }

    let mut builder = if let Some(m) = mutation {
        if has_subscription {
            Schema::build(
                query.type_name(),
                Some(m.type_name()),
                Some(subscription.type_name()),
            )
            .register(query)
            .register(m)
            .register(subscription)
        } else {
            Schema::build(query.type_name(), Some(m.type_name()), None)
                .register(query)
                .register(m)
        }
    } else if has_subscription {
        Schema::build(query.type_name(), None, Some(subscription.type_name()))
            .register(query)
            .register(subscription)
    } else {
        Schema::build(query.type_name(), None, None).register(query)
    };

    builder = builder.register(order_by_enum);
    for name in CUSTOM_SCALARS {
        builder = builder.register(Scalar::new(*name));
    }
    for (_, obj) in registered_objects {
        builder = builder.register(obj);
    }
    for (_, input) in registered_inputs {
        builder = builder.register(input);
    }

    let mut builder = builder
        .limit_depth(max_depth)
        .limit_complexity(max_complexity);
    if disable_introspection {
        builder = builder.disable_introspection();
    }
    builder.finish().map_err(|e: SchemaError| e.to_string())
}

fn ensure_object_type(
    objects: &mut BTreeMap<String, Object>,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    permissions: &BTreeMap<(String, String), RoleModelPerm>,
    role: &str,
    perm: &SelectPermission,
    scalars: &mut std::collections::BTreeSet<&'static str>,
) {
    let name = object_type_name(schema).to_string();
    if objects.contains_key(&name) {
        return;
    }
    // Break relationship cycles while nested object fields are registered.
    objects.insert(name.clone(), Object::new(name.clone()));
    let mut obj = Object::new(name.clone());
    for col in schema.columns.iter().filter(|c| !c.skipped) {
        if !perm.allows_column(&col.column_name) {
            continue;
        }
        let Some(scalar) = scalar_type_name(&col.column_type) else {
            continue;
        };
        scalars.insert(scalar);
        let ty = if col.nullable {
            TypeRef::named(scalar)
        } else {
            TypeRef::named_nn(scalar)
        };
        let key = col.column_name.clone();
        obj = obj.field(Field::new(col.column_name.as_str(), ty, move |ctx| {
            let key = key.clone();
            FieldFuture::new(async move { passthrough(&ctx, &key) })
        }));
    }
    for rel in &schema.relationships {
        let Some(target) = catalog.get(&rel.target_model) else {
            continue;
        };
        let Some(target_perm) = permissions.get(&(rel.target_model.clone(), role.to_string()))
        else {
            continue;
        };
        let target_obj = object_type_name(&target.schema).to_string();
        ensure_object_type(
            objects,
            &target.schema,
            catalog,
            permissions,
            role,
            &target_perm.permission,
            scalars,
        );
        let key = rel.field_name.clone();
        match rel.kind {
            RelationshipKind::BelongsTo => {
                let fk_nullable = schema
                    .columns
                    .iter()
                    .find(|c| {
                        c.column_name == rel.foreign_key.as_deref().unwrap_or("")
                            || c.field_name == rel.foreign_key.as_deref().unwrap_or("")
                    })
                    .map(|c| c.nullable)
                    .unwrap_or(true);
                let ty = if fk_nullable {
                    TypeRef::named(target_obj)
                } else {
                    TypeRef::named_nn(target_obj)
                };
                obj = obj.field(Field::new(rel.field_name.as_str(), ty, move |ctx| {
                    let key = key.clone();
                    FieldFuture::new(async move { passthrough(&ctx, &key) })
                }));
            }
            RelationshipKind::HasMany | RelationshipKind::ManyToMany => {
                let bool_exp = bool_exp_name(&target.schema);
                let order_by = order_by_name(&target.schema);
                let field = Field::new(
                    rel.field_name.as_str(),
                    TypeRef::named_nn_list_nn(target_obj),
                    move |ctx| {
                        let key = key.clone();
                        FieldFuture::new(async move { passthrough(&ctx, &key) })
                    },
                )
                .argument(InputValue::new("where", TypeRef::named(bool_exp.clone())))
                .argument(InputValue::new(
                    "order_by",
                    TypeRef::named_nn_list(order_by),
                ))
                .argument(InputValue::new("limit", TypeRef::named(TypeRef::INT)))
                .argument(InputValue::new("offset", TypeRef::named(TypeRef::INT)));
                obj = obj.field(field);
                if target_perm.permission.allow_aggregations {
                    ensure_aggregate_type(objects, &target.schema);
                    let agg_key = format!("{}_aggregate", rel.field_name);
                    let agg_type = format!("{}_aggregate", target.schema.table_name);
                    let field_key = agg_key.clone();
                    let agg_field = Field::new(agg_key, TypeRef::named(agg_type), move |ctx| {
                        let key = field_key.clone();
                        FieldFuture::new(async move { passthrough(&ctx, &key) })
                    })
                    .argument(InputValue::new("where", TypeRef::named(bool_exp)));
                    obj = obj.field(agg_field);
                }
            }
        }
    }
    objects.insert(name, obj);
}

fn ensure_bool_exp(
    inputs: &mut BTreeMap<String, InputObject>,
    schema: &TableSchema,
    catalog: &BTreeMap<String, CatalogEntry>,
    permissions: &BTreeMap<(String, String), RoleModelPerm>,
    role: &str,
    perm: &SelectPermission,
    scalars: &mut std::collections::BTreeSet<&'static str>,
) {
    let name = bool_exp_name(schema);
    if inputs.contains_key(&name) {
        return;
    }
    // Insert placeholder first to break cycles.
    inputs.insert(name.clone(), InputObject::new(name.clone()));
    let mut input = InputObject::new(name.clone());
    // Optional list/recursive fields (Hasura-style); never required.
    input = input.field(InputValue::new(
        "_and",
        TypeRef::named_nn_list(name.as_str()),
    ));
    input = input.field(InputValue::new(
        "_or",
        TypeRef::named_nn_list(name.as_str()),
    ));
    input = input.field(InputValue::new("_not", TypeRef::named(name.as_str())));
    for col in schema.columns.iter().filter(|c| !c.skipped) {
        if !perm.allows_column(&col.column_name) {
            continue;
        }
        if let Some(scalar) = scalar_type_name(&col.column_type) {
            scalars.insert(scalar);
            let cmp = comparison_exp_name(scalar);
            input = input.field(InputValue::new(
                col.column_name.as_str(),
                TypeRef::named(cmp),
            ));
        }
    }
    for rel in &schema.relationships {
        if catalog.get(&rel.target_model).is_none() {
            continue;
        }
        if permissions
            .get(&(rel.target_model.clone(), role.to_string()))
            .is_none()
        {
            continue;
        }
        let target = &catalog[&rel.target_model].schema;
        let target_perm = &permissions[&(rel.target_model.clone(), role.to_string())].permission;
        ensure_bool_exp(
            inputs,
            target,
            catalog,
            permissions,
            role,
            target_perm,
            scalars,
        );
        let target_bool = bool_exp_name(target);
        input = input.field(InputValue::new(
            rel.field_name.as_str(),
            TypeRef::named(target_bool),
        ));
    }
    inputs.insert(name, input);
}

fn ensure_order_by_input(
    inputs: &mut BTreeMap<String, InputObject>,
    schema: &TableSchema,
    perm: &SelectPermission,
) {
    let name = order_by_name(schema);
    if inputs.contains_key(&name) {
        return;
    }
    let mut input = InputObject::new(name.clone());
    for col in schema.columns.iter().filter(|c| !c.skipped) {
        if !perm.allows_column(&col.column_name) {
            continue;
        }
        input = input.field(InputValue::new(
            col.column_name.as_str(),
            TypeRef::named("order_by"),
        ));
    }
    inputs.insert(name, input);
}

fn ensure_aggregate_type(objects: &mut BTreeMap<String, Object>, schema: &TableSchema) {
    let agg = format!("{}_aggregate", schema.table_name);
    if objects.contains_key(&agg) {
        return;
    }
    let fields_name = format!("{}_aggregate_fields", schema.table_name);
    let mut fields_obj = Object::new(fields_name.clone());
    fields_obj = fields_obj.field(Field::new(
        "count",
        TypeRef::named_nn(TypeRef::INT),
        |ctx| FieldFuture::new(async move { passthrough(&ctx, "count") }),
    ));
    objects.insert(fields_name.clone(), fields_obj);

    let mut agg_obj = Object::new(agg.clone());
    agg_obj = agg_obj.field(Field::new(
        "aggregate",
        TypeRef::named(fields_name),
        |ctx| FieldFuture::new(async move { passthrough(&ctx, "aggregate") }),
    ));
    let obj = object_type_name(schema).to_string();
    agg_obj = agg_obj.field(Field::new("nodes", TypeRef::named_nn_list_nn(obj), |ctx| {
        FieldFuture::new(async move { passthrough(&ctx, "nodes") })
    }));
    objects.insert(agg, agg_obj);
}

fn ensure_command_input(
    inputs: &mut BTreeMap<String, InputObject>,
    tdef: &super::types::GraphqlTypeDef,
) {
    if inputs.contains_key(&tdef.name) {
        return;
    }
    let mut input = InputObject::new(tdef.name.clone());
    for field in &tdef.fields {
        let ty = match (field.list, field.nullable) {
            (true, false) => TypeRef::named_nn_list_nn(field.type_name.as_str()),
            (true, true) => TypeRef::named_nn_list(field.type_name.as_str()),
            (false, false) => TypeRef::named_nn(field.type_name.as_str()),
            (false, true) => TypeRef::named(field.type_name.as_str()),
        };
        input = input.field(InputValue::new(field.name.as_str(), ty));
        if let Some(nested) = &field.nested {
            ensure_command_input(inputs, nested);
        }
    }
    inputs.insert(tdef.name.clone(), input);
}

fn passthrough(
    ctx: &async_graphql::dynamic::ResolverContext<'_>,
    key: &str,
) -> Result<Option<Value>, async_graphql::Error> {
    let Some(value) = ctx.parent_value.as_value() else {
        return Ok(None);
    };
    // Nested JSON may arrive as a string (SQLite json_group_array quirk).
    if let Value::String(s) = value {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
            if let Ok(v) = Value::from_json(parsed) {
                return Ok(lookup_key(&v, key));
            }
        }
    }
    Ok(lookup_key(value, key))
}

fn lookup_key(value: &Value, key: &str) -> Option<Value> {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k.as_str() == key {
                    return Some(v.clone());
                }
            }
            None
        }
        _ => None,
    }
}

async fn resolve_root(
    ctx: &async_graphql::dynamic::ResolverContext<'_>,
    model: &str,
    kind: RootKind,
) -> Result<Option<Value>, async_graphql::Error> {
    let inner = ctx
        .data_opt::<Arc<EngineInner>>()
        .cloned()
        .ok_or_else(|| async_graphql::Error::new("GraphqlEngine not in request data"))?;
    let session = ctx
        .data_opt::<Session>()
        .cloned()
        .unwrap_or_else(Session::new);
    let role = session
        .role()
        .map(|s| s.to_string())
        .unwrap_or_else(|| inner.anonymous_role.clone());

    let selection = compile::selection_from_field(ctx.field());
    let plan = compile::compile_root(&inner, &session, &role, model, kind, &selection)
        .map_err(|e| client_error("BAD_REQUEST", sanitize_compile_error(&e)))?;
    let value = super::engine::execute_plan(&inner, &plan)
        .await
        .map_err(|e| client_error_for_execute_err(&e))?;
    // `None` (not `Some(Null)`) so nullable by_pk roots do not try to resolve
    // non-null child fields on a null parent.
    if matches!(value, Value::Null) {
        Ok(None)
    } else {
        Ok(Some(value))
    }
}

/// Map executor error strings to stable client errors (`extensions.code`).
pub(crate) fn client_error_for_execute_err(e: &str) -> async_graphql::Error {
    if e.contains("timeout") {
        client_error("TIMEOUT", "statement timeout")
    } else {
        client_error("INTERNAL", "internal error")
    }
}

fn sanitize_compile_error(e: &str) -> String {
    // Stable short messages; never return raw SQL.
    if e.contains("max depth") {
        "max depth exceeded".into()
    } else if e.contains("too complex") || e.contains("query too complex") {
        "query too complex".into()
    } else if e.contains("max_in_list")
        || e.contains("_in list")
        || e.contains("max_bool_width")
        || e.contains("_and list")
        || e.contains("_or list")
    {
        "list too long".into()
    } else if e.contains("invalid GraphQL response key") {
        "invalid response key".into()
    } else if e.contains("unknown comparison")
        || e.contains("unknown where field")
        || e.contains("ungranted where")
        || e.contains("unknown order_by")
        || e.contains("ungranted order_by")
    {
        "invalid filter".into()
    } else {
        "bad request".into()
    }
}

fn client_error(code: &str, message: impl Into<String>) -> async_graphql::Error {
    use async_graphql::ErrorExtensions;
    let code = code.to_string();
    async_graphql::Error::new(message.into()).extend_with(move |_, ext| {
        ext.set("code", code.as_str());
    })
}

#[cfg(test)]
mod execute_err_mapping_tests {
    use super::{client_error_for_execute_err, sanitize_compile_error};

    #[test]
    fn statement_timeout_maps_to_timeout_code() {
        let err = client_error_for_execute_err("statement timeout");
        assert_eq!(err.message, "statement timeout");
        let code = err
            .extensions
            .as_ref()
            .and_then(|ext| ext.get("code"))
            .map(|v| format!("{v:?}"));
        assert!(
            code.as_deref()
                .map(|c| c.contains("TIMEOUT"))
                .unwrap_or(false),
            "expected TIMEOUT extension, got {code:?}"
        );
    }

    #[test]
    fn other_errors_map_to_internal() {
        let err = client_error_for_execute_err("sqlite execute: boom");
        assert_eq!(err.message, "internal error");
        let code = err
            .extensions
            .as_ref()
            .and_then(|ext| ext.get("code"))
            .map(|v| format!("{v:?}"));
        assert!(
            code.as_deref()
                .map(|c| c.contains("INTERNAL"))
                .unwrap_or(false),
            "expected INTERNAL extension, got {code:?}"
        );
    }

    #[test]
    fn sanitize_compile_error_table() {
        assert_eq!(sanitize_compile_error("max depth exceeded"), "max depth exceeded");
        assert_eq!(
            sanitize_compile_error("_and list length 999 exceeds max_bool_width 256"),
            "list too long"
        );
        assert_eq!(
            sanitize_compile_error("_in list length 500 exceeds max_in_list 100"),
            "list too long"
        );
        assert_eq!(
            sanitize_compile_error("invalid GraphQL response key `x y`"),
            "invalid response key"
        );
        assert_eq!(
            sanitize_compile_error("unknown comparison op `_wat`"),
            "invalid filter"
        );
        assert_eq!(
            sanitize_compile_error("unknown where field `nope`"),
            "invalid filter"
        );
        assert_eq!(
            sanitize_compile_error("ungranted order_by column `secret`"),
            "invalid filter"
        );
        assert_eq!(sanitize_compile_error("SELECT * FROM secret"), "bad request");
    }
}

async fn resolve_command(
    ctx: &async_graphql::dynamic::ResolverContext<'_>,
    command_name: &str,
) -> Result<Option<Value>, async_graphql::Error> {
    use crate::microsvc::{CommandRequest, Service};

    let session = ctx
        .data_opt::<Session>()
        .cloned()
        .unwrap_or_else(Session::new);
    let service = ctx.data_opt::<Arc<Service>>();
    let Some(service) = service else {
        return Err(async_graphql::Error::new(
            "command dispatcher not configured (use graphql_router_with_service)",
        ));
    };

    let input = ctx
        .args
        .get("input")
        .map(|v| v.deserialize::<serde_json::Value>())
        .transpose()
        .map_err(|e| async_graphql::Error::new(format!("{e:?}")))?
        .unwrap_or(serde_json::json!({}));

    let request = CommandRequest {
        command: command_name.to_string(),
        input,
        session_variables: session.variables().clone(),
    };
    let response = service.dispatch_request(&request).await;
    // Map status → GraphQL error codes
    if response.status >= 400 {
        let msg = if response.status >= 500 {
            "internal error".to_string()
        } else {
            response
                .body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("request failed")
                .to_string()
        };
        return Err(async_graphql::Error::new(format!(
            "{msg} [{}]",
            status_code_name(response.status)
        )));
    }
    Value::from_json(response.body)
        .map(Some)
        .map_err(|e| async_graphql::Error::new(format!("response encode: {e}")))
}

fn status_code_name(status: u16) -> &'static str {
    match status {
        400 => "BAD_REQUEST",
        401 => "UNAUTHORIZED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        409 => "CONFLICT",
        _ if status >= 500 => "INTERNAL",
        _ => "BAD_REQUEST",
    }
}
