use std::sync::RwLock;

use crate::read_model::ReadModelLoadRequest;
use crate::table::{
    RelationshipDef, RelationshipKind, TableSchema, TableSchemaRegistry, TableStoreError,
};

/// A resolved relationship include: the relationship metadata plus the registered
/// schema of the target model, ready for the relational load path to query.
#[derive(Clone)]
pub(crate) struct IncludeSpec {
    pub(crate) name: String,
    pub(crate) relationship: RelationshipDef,
    pub(crate) target_schema: TableSchema,
}

pub(crate) fn remember_read_model_schemas(
    stored: &RwLock<TableSchemaRegistry>,
    registry: &TableSchemaRegistry,
) -> Result<(), TableStoreError> {
    let mut stored = stored
        .write()
        .map_err(|_| TableStoreError::Storage("read-model schema registry lock poisoned".into()))?;

    for schema in registry.schemas() {
        if let Some(existing) = stored.schema_for_table(&schema.table_name) {
            if existing != schema {
                return Err(TableStoreError::Metadata(format!(
                    "read-model schema registry already contains table `{}` with different metadata",
                    schema.table_name
                )));
            }
            continue;
        }
        stored.register_schema(schema.clone())?;
    }

    Ok(())
}

pub(crate) fn resolve_registered_read_model_schemas(
    registry: &RwLock<TableSchemaRegistry>,
    request: &ReadModelLoadRequest,
) -> Result<(TableSchema, Vec<IncludeSpec>), TableStoreError> {
    if request.includes.is_empty() {
        return Ok((request.schema.clone(), Vec::new()));
    }

    let registry = registry
        .read()
        .map_err(|_| TableStoreError::Storage("read-model schema registry lock poisoned".into()))?;
    let root_schema = registry
        .schema_for_model(&request.schema.model_name)
        .cloned()
        .ok_or_else(|| {
            TableStoreError::Metadata(format!(
                "read model `{}` is not registered for relationship includes",
                request.schema.model_name
            ))
        })?;
    if root_schema != request.schema {
        return Err(TableStoreError::Metadata(format!(
            "read model `{}` load request does not match registered schema",
            request.schema.model_name
        )));
    }

    let mut include_specs = Vec::with_capacity(request.includes.len());
    for include_name in &request.includes {
        let relationship = root_schema
            .relationships
            .iter()
            .find(|relationship| relationship.field_name == *include_name)
            .ok_or_else(|| {
                TableStoreError::Metadata(format!(
                    "read model `{}` has no relationship `{}`",
                    root_schema.model_name, include_name
                ))
            })?;
        if matches!(relationship.kind, RelationshipKind::ManyToMany) {
            return Err(TableStoreError::Metadata(format!(
                "many-to-many relationship `{}` includes are not supported by the ORM include loader (join metadata may declare source and target keys; the GraphQL engine traverses m2m independently)",
                relationship.field_name
            )));
        }
        let target_schema = registry
            .schema_for_model(&relationship.target_model)
            .ok_or_else(|| {
                TableStoreError::Metadata(format!(
                    "read model `{}` relationship `{}` targets unregistered model `{}`",
                    root_schema.model_name, relationship.field_name, relationship.target_model
                ))
            })?;

        include_specs.push(IncludeSpec {
            name: include_name.clone(),
            relationship: relationship.clone(),
            target_schema: target_schema.clone(),
        });
    }

    Ok((root_schema, include_specs))
}
