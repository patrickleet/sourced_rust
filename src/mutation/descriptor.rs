//! Build `ProjectionDescriptor` factories whose **program and resolve** path
//! are mutation IR (event-independent programs + portable bindings).
//!
//! Physical ORM lowering still uses the existing `lower_model_mutation`
//! helpers so server execution stays authoritative and shared.

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

/// One event arm: a portable binding of one exact event contract to one
/// mutation program.
pub struct MutationProjectionArm {
    /// Stable arm id used in the dual-path projection program.
    pub arm_id: &'static str,
    /// Event → mutation input binding (selector + program).
    pub binding: MutationEventBinding,
}

/// Build a dual-path `ProjectionProgram` whose arms are mutation rewrites.
///
/// # Errors
///
/// Propagates rewrite and projection validation failures.
pub fn program_from_mutation_arms(
    name: &str,
    version: u64,
    partition: ProjectionPartition,
    arms: &[MutationProjectionArm],
) -> Result<ProjectionProgram, MutationProgramError> {
    let mut projection_arms = Vec::with_capacity(arms.len());
    for arm in arms {
        projection_arms.push(arm.binding.to_projection_arm(arm.arm_id)?);
    }
    ProjectionProgram::try_new(name, version, partition, projection_arms).map_err(Into::into)
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
/// [`program_from_mutation_arms`] / [`resolve_mutation_program`]). Callers that
/// pass `projection!`-generated factories are not using the mutation path.
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

/// Helper: assert a program was built from mutation arms (no empty ops).
pub fn assert_mutation_backed_program(
    program: &ProjectionProgram,
) -> Result<(), MutationProgramError> {
    if program.arms().is_empty() {
        return Err(MutationProgramError::InvalidOperation {
            operation: program.name().to_owned(),
            reason: "mutation-backed program requires at least one arm".to_owned(),
        });
    }
    for arm in program.arms() {
        if arm.operations().is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: arm.arm_id().to_owned(),
                reason: "mutation-backed arm has no operations".to_owned(),
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
