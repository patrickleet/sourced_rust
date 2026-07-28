use std::collections::{BTreeMap, BTreeSet};

use crate::projection::catalog::{ActiveProjectionBindings, ProjectionCatalog, ProjectionEpoch};
use crate::projection::placement::{
    ProjectionBinding, ProjectionBindingId, ProjectionBindingState, ProjectionExecutionClass,
    ProjectionPlacement,
};
use crate::{
    ProjectionField, ProjectionInvalidation, ProjectionKeyField, ProjectionMutationKind,
    ProjectionOperation, ProjectionProgram, ProjectionProgramId, ProjectionRelationshipEffect,
};

use super::types::SurfaceProjectionOwnerKind;
use super::{SurfaceModel, SurfaceRelationshipKeys};

/// One role-safe selected operation from an authoritative projection program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceProjectionOperation {
    pub operation_id: String,
    pub staging_ordinal: u32,
    pub kind: ProjectionMutationKind,
    pub model: String,
    pub storage: String,
    pub key: Vec<ProjectionKeyField>,
    pub fields: Vec<ProjectionField>,
    pub relationship_effects: Vec<ProjectionRelationshipEffect>,
    pub invalidations: Vec<ProjectionInvalidation>,
    pub force_revalidate: bool,
}

/// One exact selected event arm. Its selector is server-only; client export
/// receives only a digest-derived event reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceProjectionArm {
    pub arm_id: String,
    pub selector: crate::ProjectionEventSelector,
    pub operations: Vec<SurfaceProjectionOperation>,
}

/// Role-safe program inventory retained after Surface selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SurfaceSelectedProjectionProgram {
    pub name: String,
    pub version: u64,
    pub ir_version: u16,
    pub operation_semantics_version: u16,
    pub arms: Vec<SurfaceProjectionArm>,
}

/// One exact modeled projection registration carried through Surface
/// authorization.
///
/// On the catalog Surface the private raw tuple is present for validation.
/// Role/application selection replaces it with a field-filtered descriptor and
/// drops the raw program, binding schemas, event body paths for denied fields,
/// executor route, and physical topology.
#[derive(Clone, PartialEq, Eq)]
pub struct SurfaceModeledProjection {
    program_id: ProjectionProgramId,
    binding_id: ProjectionBindingId,
    owner: String,
    placement: ProjectionPlacement,
    execution_class: ProjectionExecutionClass,
    state: ProjectionBindingState,
    epoch: ProjectionEpoch,
    output_models: Vec<String>,
    raw_program: Option<ProjectionProgram>,
    raw_binding: Option<ProjectionBinding>,
    selected: Option<SurfaceSelectedProjectionProgram>,
}

impl std::fmt::Debug for SurfaceModeledProjection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SurfaceModeledProjection")
            .field("program_id", &self.program_id)
            .field("binding_id", &self.binding_id)
            .field("placement", &self.placement)
            .field("execution_class", &self.execution_class)
            .field("state", &self.state)
            .field("epoch", &self.epoch.as_str())
            .field("output_models", &self.output_models)
            .finish_non_exhaustive()
    }
}

impl SurfaceModeledProjection {
    #[cfg(test)]
    pub(crate) fn selected_for_client_manifest_test(
        program_id: ProjectionProgramId,
        binding_id: ProjectionBindingId,
        placement: ProjectionPlacement,
        execution_class: ProjectionExecutionClass,
        state: ProjectionBindingState,
        output_models: Vec<String>,
        selected: Option<SurfaceSelectedProjectionProgram>,
    ) -> Self {
        Self {
            program_id,
            binding_id,
            owner: "client-manifest-test".into(),
            placement,
            execution_class,
            state,
            epoch: ProjectionEpoch::new("client-manifest-test-v1")
                .expect("test epoch is canonical"),
            output_models,
            raw_program: None,
            raw_binding: None,
            selected,
        }
    }

    /// Resolve one exact registration through a validated catalog and active
    /// binding view.
    ///
    /// This is the only public authority path. Arbitrary
    /// `ProjectionBindingActivation::new(...)` values cannot bypass catalog
    /// writer-conflict, route, placement, or epoch-takeover validation.
    pub fn try_from_catalog(
        program: ProjectionProgram,
        catalog: &ProjectionCatalog,
        active: &ActiveProjectionBindings,
        binding_id: ProjectionBindingId,
    ) -> Result<Self, String> {
        let active_bytes = active
            .canonical_bytes()
            .map_err(|error| error.to_string())?;
        let validated = ActiveProjectionBindings::from_canonical_bytes(catalog, &active_bytes)
            .map_err(|error| error.to_string())?;
        let binding = catalog
            .binding(binding_id)
            .ok_or_else(|| format!("unknown modeled projection binding `{binding_id}`"))?
            .clone();
        let activation = validated
            .bindings()
            .iter()
            .find(|activation| activation.binding_id() == binding_id)
            .ok_or_else(|| format!("modeled projection binding `{binding_id}` is not live"))?;
        let program_id = program.id().map_err(|error| error.to_string())?;
        if binding.program_id() != program_id || activation.program_id() != program_id {
            return Err("modeled projection binding does not match its program digest".into());
        }
        let output_models = binding
            .outputs()
            .iter()
            .map(|output| output.model().to_owned())
            .collect();
        Ok(Self {
            program_id,
            binding_id,
            owner: binding.owner().name().to_owned(),
            placement: binding.placement(),
            execution_class: binding.execution_class(),
            state: activation.state(),
            epoch: activation.epoch().clone(),
            output_models,
            raw_program: Some(program),
            raw_binding: Some(binding),
            selected: None,
        })
    }

    /// Return the semantic program identity.
    pub fn program_id(&self) -> ProjectionProgramId {
        self.program_id
    }

    /// Return the exact deployment binding identity.
    pub fn binding_id(&self) -> ProjectionBindingId {
        self.binding_id
    }

    /// Return active or draining state.
    pub fn state(&self) -> ProjectionBindingState {
        self.state
    }

    /// Return the exact physical incarnation.
    pub fn epoch(&self) -> &ProjectionEpoch {
        &self.epoch
    }

    /// Whether this exact selected registration may mint client causal work.
    pub fn is_causally_eligible(&self) -> bool {
        self.state == ProjectionBindingState::Active
            && self.placement == ProjectionPlacement::Eventual
            && self.execution_class == ProjectionExecutionClass::Causal
    }

    pub(crate) fn placement(&self) -> ProjectionPlacement {
        self.placement
    }

    pub(crate) fn execution_class(&self) -> ProjectionExecutionClass {
        self.execution_class
    }

    pub(crate) fn output_models(&self) -> &[String] {
        &self.output_models
    }

    pub(crate) fn event_names(&self) -> Vec<String> {
        self.raw_program
            .as_ref()
            .map(|program| {
                program
                    .arms()
                    .iter()
                    .map(|arm| arm.selector().event_name().to_owned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect()
            })
            .or_else(|| {
                self.selected.as_ref().map(|program| {
                    program
                        .arms
                        .iter()
                        .map(|arm| arm.selector.event_name().to_owned())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    pub(crate) fn selected_program(&self) -> Option<&SurfaceSelectedProjectionProgram> {
        self.selected.as_ref()
    }

    pub(super) fn validate_for_surface(
        &self,
        owner_name: &str,
        kind: SurfaceProjectionOwnerKind,
        models: &BTreeMap<String, SurfaceModel>,
    ) -> Result<(), String> {
        let (program, binding) = self.raw().ok_or_else(|| {
            "selected modeled projection cannot be reattached to a catalog Surface".to_owned()
        })?;
        if self.owner != owner_name {
            return Err(format!(
                "modeled projection owner `{owner_name}` differs from binding owner `{}`",
                self.owner
            ));
        }
        let expected_placement = match kind {
            SurfaceProjectionOwnerKind::Async => ProjectionPlacement::Eventual,
            SurfaceProjectionOwnerKind::Direct => ProjectionPlacement::Direct,
        };
        if binding.placement() != expected_placement {
            return Err(format!(
                "modeled projection owner `{owner_name}` has incompatible {:?} placement",
                binding.placement()
            ));
        }
        let output_models = binding
            .outputs()
            .iter()
            .map(|output| output.model())
            .collect::<BTreeSet<_>>();
        for output in binding.outputs() {
            let Some(model) = models.get(output.model()) else {
                return Err(format!(
                    "modeled projection owner `{owner_name}` targets unknown surface model `{}`",
                    output.model()
                ));
            };
            if output.storage() != model.table_name || output.schema() != &model.schema {
                return Err(format!(
                    "modeled projection owner `{owner_name}` output `{}` does not match the authoritative Surface schema",
                    output.model()
                ));
            }
        }
        for arm in program.arms() {
            for operation in arm.operations() {
                if !output_models.contains(operation.target().model()) {
                    return Err(format!(
                        "modeled projection owner `{owner_name}` operation `{}` targets an undeclared binding output",
                        operation.operation_id()
                    ));
                }
            }
        }
        if kind == SurfaceProjectionOwnerKind::Direct {
            validate_direct_program(owner_name, program, binding, models)?;
        }
        Ok(())
    }

    pub(super) fn select_for_models(
        &self,
        models: &BTreeMap<String, SurfaceModel>,
    ) -> Result<Option<Self>, String> {
        let (program, _) = self
            .raw()
            .ok_or_else(|| "modeled projection was already Surface-selected".to_owned())?;
        let output_models = self
            .output_models
            .iter()
            .filter(|model| models.contains_key(*model))
            .cloned()
            .collect::<Vec<_>>();
        if output_models.is_empty() {
            return Ok(None);
        }
        let selected = if self.placement == ProjectionPlacement::Direct {
            None
        } else {
            let mut arms = Vec::new();
            for arm in program.arms() {
                let operations = arm
                    .operations()
                    .iter()
                    .filter_map(|operation| select_operation(operation, models))
                    .collect::<Vec<_>>();
                if !operations.is_empty() {
                    arms.push(SurfaceProjectionArm {
                        arm_id: arm.arm_id().to_owned(),
                        selector: arm.selector().clone(),
                        operations,
                    });
                }
            }
            Some(SurfaceSelectedProjectionProgram {
                name: program.name().to_owned(),
                version: program.version(),
                ir_version: program.ir_version(),
                operation_semantics_version: program.operation_semantics_version(),
                arms,
            })
        };
        Ok(Some(Self {
            output_models,
            raw_program: None,
            raw_binding: None,
            selected,
            ..self.clone()
        }))
    }

    fn raw(&self) -> Option<(&ProjectionProgram, &ProjectionBinding)> {
        Some((self.raw_program.as_ref()?, self.raw_binding.as_ref()?))
    }
}

fn select_operation(
    operation: &ProjectionOperation,
    models: &BTreeMap<String, SurfaceModel>,
) -> Option<SurfaceProjectionOperation> {
    let model = models.get(operation.target().model())?;
    if operation.target().storage() != model.table_name {
        return None;
    }
    let key_visible = operation
        .key()
        .iter()
        .all(|key| logical_field_visible(model, key.name()));
    let fields = operation
        .fields()
        .iter()
        .filter(|field| logical_field_visible(model, field.name()))
        .cloned()
        .collect::<Vec<_>>();
    let mut relationship_effects = Vec::new();
    let mut relationship_recovery = Vec::new();
    for effect in operation.relationship_effects() {
        if relationship_visible(&effect, models) {
            relationship_effects.push(effect.clone());
            continue;
        }
        if relationship_surface_visible(effect, models) {
            let relationship = effect.relationship();
            let source = models
                .get(relationship.source_model())
                .expect("visible relationship source model");
            if !effect.source_key().is_empty()
                && effect
                    .source_key()
                    .iter()
                    .all(|key| logical_field_visible(source, key.name()))
            {
                relationship_effects.push(
                    ProjectionRelationshipEffect::invalidate(
                        effect.ordinal(),
                        relationship.clone(),
                        effect.source_key().to_vec(),
                    )
                    .expect("selected source key remains complete"),
                );
                relationship_recovery.push(
                    ProjectionInvalidation::relationship(
                        relationship.source_model(),
                        relationship.relationship(),
                        relationship.target_model(),
                    )
                    .expect("selected relationship identity is non-empty"),
                );
            } else {
                relationship_recovery.push(
                    ProjectionInvalidation::model(relationship.source_model())
                        .expect("selected relationship source identity is non-empty"),
                );
            }
        }
    }
    let mut invalidations = operation
        .invalidations()
        .iter()
        .filter(|invalidation| invalidation_visible(invalidation, models))
        .cloned()
        .collect::<Vec<_>>();
    invalidations.extend(relationship_recovery);
    invalidations.sort();
    invalidations.dedup();
    let row_consequence = operation.kind() == ProjectionMutationKind::Delete || !fields.is_empty();
    let hidden_only_row_change = operation.kind() != ProjectionMutationKind::Delete
        && !operation.fields().is_empty()
        && fields.is_empty();
    let force_revalidate = (row_consequence && !key_visible) || hidden_only_row_change;
    if force_revalidate {
        invalidations.push(
            ProjectionInvalidation::model(&model.model_name)
                .expect("selected model identity is non-empty"),
        );
        invalidations.sort();
        invalidations.dedup();
    }
    Some(SurfaceProjectionOperation {
        operation_id: operation.operation_id().to_owned(),
        staging_ordinal: operation.staging_ordinal(),
        kind: operation.kind(),
        model: operation.target().model().to_owned(),
        storage: operation.target().storage().to_owned(),
        key: key_visible
            .then(|| operation.key().to_vec())
            .unwrap_or_default(),
        fields: if force_revalidate { Vec::new() } else { fields },
        relationship_effects,
        invalidations,
        force_revalidate,
    })
}

fn logical_field_visible(model: &SurfaceModel, logical: &str) -> bool {
    model
        .schema
        .columns
        .iter()
        .find(|column| !column.skipped && column.field_name == logical)
        .is_some_and(|column| {
            model
                .columns
                .iter()
                .any(|selected| selected.name == column.column_name)
        })
}

fn relationship_visible(
    effect: &&ProjectionRelationshipEffect,
    models: &BTreeMap<String, SurfaceModel>,
) -> bool {
    let relationship = effect.relationship();
    let Some(source) = models.get(relationship.source_model()) else {
        return false;
    };
    let Some(target) = models.get(relationship.target_model()) else {
        return false;
    };
    let Some(selected) = source
        .relationships
        .iter()
        .find(|selected| selected.name == relationship.relationship())
    else {
        return false;
    };
    if selected.target_model != target.model_name
        || matches!(selected.keys, SurfaceRelationshipKeys::Embedded)
    {
        return false;
    }
    effect
        .source_key()
        .iter()
        .all(|key| logical_field_visible(source, key.name()))
        && effect
            .target_key()
            .iter()
            .all(|key| logical_field_visible(target, key.name()))
}

fn relationship_surface_visible(
    effect: &ProjectionRelationshipEffect,
    models: &BTreeMap<String, SurfaceModel>,
) -> bool {
    let relationship = effect.relationship();
    models
        .get(relationship.source_model())
        .is_some_and(|source| {
            source.relationships.iter().any(|selected| {
                selected.name == relationship.relationship()
                    && selected.target_model == relationship.target_model()
                    && models.contains_key(relationship.target_model())
            })
        })
}

fn invalidation_visible(
    invalidation: &&ProjectionInvalidation,
    models: &BTreeMap<String, SurfaceModel>,
) -> bool {
    match invalidation {
        ProjectionInvalidation::Model { model } => models.contains_key(model),
        ProjectionInvalidation::Relationship {
            source_model,
            relationship,
            target_model,
        } => models.get(source_model).is_some_and(|source| {
            source.relationships.iter().any(|selected| {
                selected.name == *relationship && selected.target_model == *target_model
            })
        }),
    }
}

fn validate_direct_program(
    owner: &str,
    program: &ProjectionProgram,
    binding: &ProjectionBinding,
    models: &BTreeMap<String, SurfaceModel>,
) -> Result<(), String> {
    let [output] = binding.outputs() else {
        return Err(format!(
            "direct modeled projection `{owner}` must own exactly one output model"
        ));
    };
    let model = models
        .get(output.model())
        .expect("modeled output presence was validated above");
    let complete_fields = model
        .schema
        .columns
        .iter()
        .filter(|column| !column.skipped)
        .map(|column| column.field_name.as_str())
        .collect::<BTreeSet<_>>();
    let logical_primary_key = model
        .schema
        .primary_key
        .columns
        .iter()
        .map(|physical| {
            model
                .schema
                .columns
                .iter()
                .find(|column| !column.skipped && column.column_name == *physical)
                .map(|column| column.field_name.as_str())
                .ok_or_else(|| {
                    format!(
                        "direct modeled projection `{owner}` cannot map physical primary key `{physical}` to one logical field"
                    )
                })
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    for arm in program.arms() {
        let [operation] = arm.operations() else {
            return Err(format!(
                "direct modeled projection `{owner}` arm `{}` must contain exactly one operation",
                arm.arm_id()
            ));
        };
        let operation_fields = operation
            .fields()
            .iter()
            .map(|field| field.name())
            .collect::<BTreeSet<_>>();
        let operation_key = operation
            .key()
            .iter()
            .map(|field| field.name())
            .collect::<BTreeSet<_>>();
        if operation.kind() != ProjectionMutationKind::Upsert
            || operation.target().model() != output.model()
            || operation.target().storage() != output.storage()
            || operation_fields != complete_fields
            || operation_key != logical_primary_key
        {
            return Err(format!(
                "direct modeled projection `{owner}` arm `{}` is not one complete logical full-row upsert",
                arm.arm_id()
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::{build_surface, surface_for_role, RoleGrant, SurfaceOptions};
    use crate::table::{
        ColumnType, PrimaryKey, RelationshipDef, RelationshipKind, TableColumn, TableKind,
        TableSchema,
    };
    use crate::{
        ProjectionAssignment, ProjectionExpression, ProjectionRelationship, ProjectionValue,
    };

    fn schema(
        model: &str,
        table: &str,
        fields: &[&str],
        relationships: Vec<RelationshipDef>,
    ) -> TableSchema {
        TableSchema {
            model_name: model.into(),
            table_name: table.into(),
            columns: fields
                .iter()
                .enumerate()
                .map(|(index, field)| TableColumn {
                    primary_key: index == 0,
                    ..TableColumn::new(*field, *field, ColumnType::Text)
                })
                .collect(),
            primary_key: PrimaryKey::new([fields[0]]),
            version_column: None,
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            relationships,
            kind: TableKind::ReadModel,
        }
    }

    fn models(grants: BTreeMap<String, RoleGrant>) -> BTreeMap<String, SurfaceModel> {
        let todo = schema(
            "TodoView",
            "todos",
            &["todo_id", "owner_id", "title"],
            vec![RelationshipDef {
                field_name: "owner".into(),
                kind: RelationshipKind::BelongsTo,
                target_model: "UserView".into(),
                foreign_key: Some("owner_id".into()),
                through: None,
                target_foreign_key: None,
            }],
        );
        let user = schema("UserView", "users", &["user_id", "name"], Vec::new());
        surface_for_role(
            &build_surface(&[todo, user], &SurfaceOptions::sqlite()).unwrap(),
            "user",
            &grants,
        )
        .unwrap()
        .models
    }

    fn key(ordinal: u32, name: &str) -> ProjectionKeyField {
        ProjectionKeyField::try_new(
            ordinal,
            name,
            ProjectionExpression::constant(ProjectionValue::string("id")),
        )
        .unwrap()
    }

    fn operation(
        field: &str,
        relationship_effects: Vec<ProjectionRelationshipEffect>,
    ) -> ProjectionOperation {
        ProjectionOperation::try_new(
            "patch-and-link",
            0,
            ProjectionMutationKind::Patch,
            crate::ProjectionTarget::try_new("TodoView", "todos").unwrap(),
            vec![key(0, "todo_id")],
            vec![ProjectionField::try_new(
                0,
                field,
                ProjectionAssignment::Set(ProjectionExpression::constant(ProjectionValue::string(
                    "changed",
                ))),
            )
            .unwrap()],
            relationship_effects,
            Vec::new(),
        )
        .unwrap()
    }

    fn owner_link(source_field: &str, target_field: &str) -> ProjectionRelationshipEffect {
        ProjectionRelationshipEffect::link(
            0,
            ProjectionRelationship::try_new("TodoView", "owner", "UserView").unwrap(),
            vec![key(0, source_field)],
            vec![key(0, target_field)],
        )
        .unwrap()
    }

    #[test]
    fn hidden_row_change_adds_model_recovery_without_erasing_safe_edge() {
        let selected = select_operation(
            &operation("title", vec![owner_link("todo_id", "user_id")]),
            &models(BTreeMap::from([
                (
                    "TodoView".into(),
                    RoleGrant::columns(["todo_id", "owner_id"]),
                ),
                ("UserView".into(), RoleGrant::columns(["user_id"])),
            ])),
        )
        .unwrap();

        assert!(selected.force_revalidate);
        assert!(selected.fields.is_empty());
        assert_eq!(selected.relationship_effects.len(), 1);
        assert!(selected
            .invalidations
            .contains(&ProjectionInvalidation::model("TodoView").unwrap()));
    }

    #[test]
    fn denied_relationship_consequence_is_omitted_without_broad_recovery() {
        let selected = select_operation(
            &operation("title", vec![owner_link("todo_id", "user_id")]),
            &models(BTreeMap::from([(
                "TodoView".into(),
                RoleGrant::all_columns(),
            )])),
        )
        .unwrap();

        assert!(!selected.force_revalidate);
        assert!(selected.relationship_effects.is_empty());
        assert!(selected.invalidations.is_empty());
        assert_eq!(selected.fields.len(), 1);
    }

    #[test]
    fn unsafe_visible_relationship_uses_narrow_or_source_model_recovery() {
        let narrow = select_operation(
            &operation("title", vec![owner_link("todo_id", "user_id")]),
            &models(BTreeMap::from([
                ("TodoView".into(), RoleGrant::all_columns()),
                ("UserView".into(), RoleGrant::columns(["name"])),
            ])),
        )
        .unwrap();
        assert_eq!(
            narrow.relationship_effects[0].kind(),
            crate::ProjectionRelationshipEffectKind::Invalidate
        );
        assert!(narrow.invalidations.contains(
            &ProjectionInvalidation::relationship("TodoView", "owner", "UserView",).unwrap()
        ));
        assert!(!narrow.force_revalidate);

        let source_recovery = select_operation(
            &operation("owner_id", vec![owner_link("title", "user_id")]),
            &models(BTreeMap::from([
                (
                    "TodoView".into(),
                    RoleGrant::columns(["todo_id", "owner_id"]),
                ),
                ("UserView".into(), RoleGrant::columns(["user_id"])),
            ])),
        )
        .unwrap();
        assert!(source_recovery.relationship_effects.is_empty());
        assert!(source_recovery
            .invalidations
            .contains(&ProjectionInvalidation::model("TodoView").unwrap()));
        assert!(!source_recovery.force_revalidate);
        assert_eq!(source_recovery.fields.len(), 1);
    }

    #[test]
    fn partial_multi_model_selection_keeps_authorized_operations_only() {
        let selected_models = models(BTreeMap::from([(
            "TodoView".into(),
            RoleGrant::all_columns(),
        )]));
        let todo = operation("title", Vec::new());
        let user = ProjectionOperation::try_new(
            "patch-user",
            1,
            ProjectionMutationKind::Patch,
            crate::ProjectionTarget::try_new("UserView", "users").unwrap(),
            vec![key(0, "user_id")],
            vec![ProjectionField::try_new(
                0,
                "name",
                ProjectionAssignment::Set(ProjectionExpression::constant(ProjectionValue::string(
                    "changed",
                ))),
            )
            .unwrap()],
            Vec::new(),
            Vec::new(),
        )
        .unwrap();

        assert_eq!(
            select_operation(&todo, &selected_models)
                .expect("authorized model operation")
                .model,
            "TodoView"
        );
        assert!(
            select_operation(&user, &selected_models).is_none(),
            "an operation against a denied output model must not survive Surface selection"
        );
    }
}
