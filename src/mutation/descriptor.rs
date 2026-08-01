//! Compile event→mutation projections for service registration.
//!
//! Authors use [`bind_state_body_to_mutation`] / [`bind_delete_to_envelope_id`]
//! and [`crate::projection!`]. Physical lowering reuses existing ORM helpers;
//! that rewrite is not part of the public author model.

use crate::projection::lower::{
    finish_lowering, lower_model_mutation, ProjectionDescriptor, ProjectionInventoryFactory,
    ProjectionLowerer, ProjectionLoweringError, ProjectionOutputInventory, ProjectionOutputModel,
    ProjectionProgramFactory, ProjectionResolver,
};
use crate::projection::{
    ProjectionPartition, ProjectionProgram, ProjectionProgramError, ResolvedProjectionPlan,
};
use crate::read_model::RelationalReadModel;
use crate::DomainEventOccurrence;

use super::bind::MutationEventBinding;
use super::MutationProgramError;

/// One event contract → one mutation program (an arm of a projection).
///
/// Prefer [`bind_state_body_to_mutation`] / [`bind_delete_to_envelope_id`].
/// Use [`ProjectionHandler::from_binding`] only when input bindings are custom.
pub struct ProjectionHandler {
    mount_id: &'static str,
    binding: MutationEventBinding,
}

impl ProjectionHandler {
    /// Build a handler from an explicit event→mutation binding.
    pub fn from_binding(mount_id: &'static str, binding: MutationEventBinding) -> Self {
        Self { mount_id, binding }
    }
}

/// Compile event→mutation arms into a service projection mount.
///
/// Prefer [`crate::projection!`] for unit partition. Use this when the
/// partition expression is custom (e.g. chat room id).
///
/// # Errors
///
/// Propagates program validation failures.
pub fn compile_projection(
    name: &str,
    version: u64,
    partition: ProjectionPartition,
    handlers: impl IntoIterator<Item = ProjectionHandler>,
) -> Result<ProjectionProgram, ProjectionProgramError> {
    let handlers: Vec<_> = handlers.into_iter().collect();
    let mut projection_arms = Vec::with_capacity(handlers.len());
    for handler in &handlers {
        projection_arms.push(
            handler
                .binding
                .to_projection_arm(handler.mount_id)
                .map_err(|error| ProjectionProgramError::InvalidOperation {
                    operation: name.into(),
                    reason: error.to_string(),
                })?,
        );
    }
    ProjectionProgram::try_new(name, version, partition, projection_arms).map_err(|error| {
        ProjectionProgramError::InvalidOperation {
            operation: name.into(),
            reason: error.to_string(),
        }
    })
}

/// Bind an exact event contract to a mutation program with explicit inputs.
///
/// # Errors
///
/// Propagates selector or binding construction failures.
pub fn bind_event_to_mutation(
    descriptor: &crate::DomainEventDescriptor,
    inputs: Vec<super::bind::MutationInputBinding>,
    program: super::program::MutationProgram,
) -> Result<ProjectionHandler, MutationProgramError> {
    let selector = crate::projection::ProjectionEventSelector::try_from_descriptor(descriptor)
        .map_err(MutationProgramError::from)?;
    let binding = super::bind::MutationEventBinding::try_new(selector, inputs, program)?;
    let mount_id = Box::leak(descriptor.name.replace('.', "-").into_boxed_str());
    Ok(ProjectionHandler { mount_id, binding })
}

/// Spec: projection arm — domain-event body → mutation input object.
///
/// When the event fires, every non-skipped model column is bound from the event
/// body into `input.{input_root}.{field}`.
///
/// # Errors
///
/// Propagates selector or binding construction failures.
pub fn bind_state_body_to_mutation<M>(
    descriptor: &crate::DomainEventDescriptor,
    program: super::program::MutationProgram,
    input_root: &str,
) -> Result<ProjectionHandler, MutationProgramError>
where
    M: RelationalReadModel,
{
    bind_event_to_mutation(
        descriptor,
        body_bindings_for_model::<M>(input_root)?,
        program,
    )
}

/// Bind several domain-event contracts to the same complete-row upsert mutation.
///
/// # Errors
///
/// Propagates the first binding construction failure.
pub fn bind_state_events_to_mutation<M>(
    program: &super::program::MutationProgram,
    input_root: &str,
    events: &[&crate::DomainEventDescriptor],
) -> Result<Vec<ProjectionHandler>, MutationProgramError>
where
    M: RelationalReadModel,
{
    events
        .iter()
        .map(|descriptor| {
            bind_state_body_to_mutation::<M>(descriptor, program.clone(), input_root)
        })
        .collect()
}

/// Spec: projection arm — deletion event → delete mutation by PK.
///
/// Fills `input.{pk_field}` from the envelope aggregate id.
///
/// # Errors
///
/// Propagates selector or binding construction failures.
pub fn bind_delete_to_envelope_id(
    descriptor: &crate::DomainEventDescriptor,
    program: super::program::MutationProgram,
    pk_field: &str,
) -> Result<ProjectionHandler, MutationProgramError> {
    let selector = crate::projection::ProjectionEventSelector::try_from_descriptor(descriptor)
        .map_err(MutationProgramError::from)?;
    let inputs = vec![super::bind::envelope_binding(
        [pk_field],
        crate::projection::ProjectionEnvelopeField::AggregateId,
    )?];
    let binding = super::bind::MutationEventBinding::try_new(selector, inputs, program)?;
    let mount_id = Box::leak(descriptor.name.replace('.', "-").into_boxed_str());
    Ok(ProjectionHandler { mount_id, binding })
}

/// Resolve an occurrence through a mutation-backed projection program.
///
/// # Errors
///
/// Propagates selector and resolution failures.
pub fn resolve_mutation_program(
    program: &ProjectionProgram,
    occurrence: &DomainEventOccurrence,
) -> Result<ResolvedProjectionPlan, ProjectionProgramError> {
    ResolvedProjectionPlan::resolve(program, occurrence)
}

/// Lower a resolved plan for a single ordinary (non-related) model `M`.
///
/// # Errors
///
/// Propagates ORM lowering failures.
pub fn lower_single_model<M>(
    plan: &ResolvedProjectionPlan,
) -> Result<crate::projection::lower::LoweredProjectionPlan, ProjectionLoweringError>
where
    M: crate::projection::lower::ProjectionReadModelMetadata,
{
    let mut builder = crate::read_model::ReadModelWritePlanBuilder::new();
    for mutation in plan.mutations() {
        lower_model_mutation::<M>(&mut builder, mutation)?;
    }
    finish_lowering(builder, plan)
}

/// Inventory for a single output model.
///
/// # Errors
///
/// Propagates schema validation failures.
pub fn inventory_single_model<M>() -> Result<ProjectionOutputInventory, ProjectionLoweringError>
where
    M: crate::projection::lower::ProjectionReadModelMetadata,
{
    Ok(ProjectionOutputInventory::new(
        vec![ProjectionOutputModel::of::<M>()?],
        Vec::new(),
    ))
}

/// Construct a const-friendly descriptor from static factories.
///
/// The factories **must** derive program/resolve from mutation IR (via
/// [`compile_projection`] / [`resolve_mutation_program`]). Prefer
/// [`crate::projection!`] or [`crate::mutation_projector!`] so the factories
/// stay mutation-backed.
pub const fn descriptor_from_factories<D>(
    name: &'static str,
    version: u64,
    epoch: &'static str,
    program: ProjectionProgramFactory,
    resolve: ProjectionResolver,
    lower: ProjectionLowerer,
    inventory: ProjectionInventoryFactory,
) -> ProjectionDescriptor<D> {
    ProjectionDescriptor::__generated(name, version, epoch, program, resolve, lower, inventory)
}

/// Assert a compiled handlers program has at least one non-empty handler.
pub fn assert_mutation_backed_program(
    program: &ProjectionProgram,
) -> Result<(), MutationProgramError> {
    if program.arms().is_empty() {
        return Err(MutationProgramError::InvalidOperation {
            operation: program.name().to_owned(),
            reason: "projection program requires at least one event binding".to_owned(),
        });
    }
    for arm in program.arms() {
        if arm.operations().is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: arm.arm_id().to_owned(),
                reason: "handler has no mutation operations".to_owned(),
            });
        }
    }
    Ok(())
}

/// Map a physical column type to the portable projection value type used in
/// mutation input paths and event body bindings.
pub fn projection_value_type_for_column(
    column_type: &crate::table::ColumnType,
) -> crate::projection::ProjectionValueType {
    use crate::projection::ProjectionValueType;
    use crate::table::ColumnType;
    match column_type {
        ColumnType::Boolean => ProjectionValueType::Boolean,
        ColumnType::Integer => ProjectionValueType::I64,
        ColumnType::UnsignedInteger => ProjectionValueType::U64,
        ColumnType::Float => ProjectionValueType::F64,
        ColumnType::Text | ColumnType::Timestamp => ProjectionValueType::String,
        ColumnType::Bytes | ColumnType::Json | ColumnType::Unsupported(_) => {
            ProjectionValueType::Json
        }
    }
}

/// Bind every non-skipped physical column of `M` as `input_root.field <- body.field`.
///
/// # Errors
///
/// Propagates binding construction failures.
pub fn body_bindings_for_model<M>(
    input_root: &str,
) -> Result<Vec<super::bind::MutationInputBinding>, MutationProgramError>
where
    M: RelationalReadModel,
{
    let schema = M::schema();
    let mut bindings = Vec::new();
    for column in schema.columns.iter().filter(|column| !column.skipped) {
        let value_type = projection_value_type_for_column(&column.column_type);
        bindings.push(super::bind::body_field_binding(
            [input_root, column.field_name.as_str()],
            [column.field_name.as_str()],
            value_type,
        )?);
    }
    Ok(bindings)
}

/// Build a complete-row upsert mutation program for model `M` from `input_root`.
///
/// # Errors
///
/// Propagates schema or validation failures.
pub fn state_upsert_program_for_model<M>(
    name: &str,
    version: u64,
    operation_id: &str,
    input_root: &str,
) -> Result<super::program::MutationProgram, MutationProgramError>
where
    M: RelationalReadModel,
{
    use super::expression::{MutationAssignment, MutationExpression};
    use super::program::{
        MutationConflictTarget, MutationField, MutationKeyField, MutationKind, MutationOperation,
        MutationProgram,
    };
    use crate::projection::{ProjectionTarget, ProjectionValueType};

    let schema = M::schema();
    let target = ProjectionTarget::try_new(schema.model_name.clone(), schema.table_name.clone())
        .map_err(MutationProgramError::from)?;
    let mut key = Vec::new();
    for (ordinal, column) in schema.primary_key.columns.iter().enumerate() {
        let path = vec![input_root.to_owned(), column.clone()];
        key.push(MutationKeyField::try_new(
            ordinal as u32,
            column.clone(),
            MutationExpression::input_path(ProjectionValueType::String, path)?,
        )?);
    }
    let mut fields = Vec::new();
    for (ordinal, column) in schema
        .columns
        .iter()
        .filter(|column| !column.skipped)
        .enumerate()
    {
        let value_type = projection_value_type_for_column(&column.column_type);
        let path = vec![input_root.to_owned(), column.field_name.clone()];
        fields.push(MutationField::try_new(
            ordinal as u32,
            column.field_name.clone(),
            MutationAssignment::set(MutationExpression::input_path(value_type, path)?),
        )?);
    }
    let op = MutationOperation::try_new(
        operation_id,
        0,
        MutationKind::Upsert,
        target,
        key,
        fields,
        Some(MutationConflictTarget::PrimaryKey),
        Vec::new(),
        Vec::new(),
        None,
    )?;
    MutationProgram::try_new(name, version, vec![op])
}

/// Build a delete-by-pk mutation program for model `M`.
///
/// # Errors
///
/// Propagates schema or validation failures.
pub fn delete_by_pk_program_for_model<M>(
    name: &str,
    version: u64,
    operation_id: &str,
) -> Result<super::program::MutationProgram, MutationProgramError>
where
    M: RelationalReadModel,
{
    use super::expression::MutationExpression;
    use super::program::{MutationKeyField, MutationKind, MutationOperation, MutationProgram};
    use crate::projection::{ProjectionTarget, ProjectionValueType};

    let schema = M::schema();
    let target = ProjectionTarget::try_new(schema.model_name.clone(), schema.table_name.clone())
        .map_err(MutationProgramError::from)?;
    let mut key = Vec::new();
    for (ordinal, column) in schema.primary_key.columns.iter().enumerate() {
        key.push(MutationKeyField::try_new(
            ordinal as u32,
            column.clone(),
            MutationExpression::input_path(ProjectionValueType::String, vec![column.clone()])?,
        )?);
    }
    let op = MutationOperation::try_new(
        operation_id,
        0,
        MutationKind::Delete,
        target,
        key,
        Vec::new(),
        None,
        Vec::new(),
        Vec::new(),
        None,
    )?;
    MutationProgram::try_new(name, version, vec![op])
}
