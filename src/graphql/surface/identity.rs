use super::*;

pub(in crate::graphql::surface) fn sanitize_relationship_identity(
    models: &mut BTreeMap<String, SurfaceModel>,
) {
    let visible_fields: BTreeMap<String, BTreeSet<String>> = models
        .iter()
        .map(|(name, model)| {
            (
                name.clone(),
                model
                    .columns
                    .iter()
                    .map(|field| field.name.clone())
                    .collect(),
            )
        })
        .collect();
    let visible_tables: BTreeMap<String, BTreeSet<String>> = models
        .values()
        .map(|model| {
            (
                model.table_name.clone(),
                model
                    .columns
                    .iter()
                    .map(|field| field.name.clone())
                    .collect(),
            )
        })
        .collect();

    for model in models.values_mut() {
        let source_fields = &visible_fields[&model.model_name];
        for relationship in &mut model.relationships {
            let target_fields = &visible_fields[&relationship.target_model];
            let keys_visible = match &relationship.keys {
                SurfaceRelationshipKeys::Direct { local, remote } => {
                    !local.is_empty()
                        && local.len() == remote.len()
                        && local.iter().all(|key| source_fields.contains(key))
                        && remote.iter().all(|key| target_fields.contains(key))
                }
                SurfaceRelationshipKeys::Through {
                    local,
                    remote,
                    table,
                    source_foreign_key,
                    target_foreign_key,
                } => {
                    let identity_visible = !local.is_empty()
                        && local.len() == remote.len()
                        && source_foreign_key.len() == local.len()
                        && target_foreign_key.len() == remote.len()
                        && local.iter().all(|key| source_fields.contains(key))
                        && remote.iter().all(|key| target_fields.contains(key));
                    if identity_visible
                        && !visible_tables.get(table).is_some_and(|fields| {
                            source_foreign_key.iter().all(|key| fields.contains(key))
                                && target_foreign_key.iter().all(|key| fields.contains(key))
                        })
                    {
                        relationship.keys = SurfaceRelationshipKeys::ThroughOpaque {
                            local: local.clone(),
                            remote: remote.clone(),
                            dependency: opaque_relationship_dependency_id(
                                &model.model_name,
                                &relationship.name,
                                &relationship.target_model,
                            ),
                        };
                    }
                    identity_visible
                }
                SurfaceRelationshipKeys::ThroughOpaque { local, remote, .. } => {
                    !local.is_empty()
                        && local.len() == remote.len()
                        && local.iter().all(|key| source_fields.contains(key))
                        && remote.iter().all(|key| target_fields.contains(key))
                }
                SurfaceRelationshipKeys::Embedded => false,
            };
            if !keys_visible {
                relationship.keys = SurfaceRelationshipKeys::Embedded;
            }

            // Visible read-model tables are already public manifest IDs. Any
            // operational dependency (notably a private many-to-many join
            // table) remains useful for conservative invalidation but is
            // represented by a stable opaque ID instead of leaking its name.
            relationship.dependencies = relationship
                .dependencies
                .iter()
                .map(|dependency| {
                    if visible_tables.contains_key(dependency)
                        || dependency.starts_with("opaque:sha256:")
                    {
                        dependency.clone()
                    } else {
                        opaque_relationship_dependency_id(
                            &model.model_name,
                            &relationship.name,
                            &relationship.target_model,
                        )
                    }
                })
                .collect();
            relationship.dependencies.sort();
            relationship.dependencies.dedup();
            if let Some(aggregate) = &mut relationship.aggregate {
                // Aggregate invalidation must never retain the pre-sanitized
                // private dependency inventory.
                aggregate.dependencies = relationship.dependencies.clone();
            }
        }
    }
}

pub(in crate::graphql::surface) fn opaque_relationship_dependency_id(
    source: &str,
    relationship: &str,
    target: &str,
) -> String {
    let material = format!(
        "distributed.client.relationship-dependency.v1\0{source}\0{relationship}\0{target}\0join"
    );
    let digest = Sha256::digest(material.as_bytes());
    format!("opaque:sha256:{digest:x}")
}
