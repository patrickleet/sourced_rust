use std::collections::BTreeSet;

use crate::projection::catalog::ProjectionBindingActivation;
use crate::projection::placement::{
    ProjectionBinding, ProjectionBindingState, ProjectionExecutionClass, ProjectionPlacement,
};
use crate::{ProjectionMutationKind, ProjectionProgram, ProjectionProgramId};

use super::types::SurfaceProjectionOwnerKind;
use super::SurfaceModel;

/// One exact modeled projection registration carried through Surface
/// authorization.
///
/// Program, deployment binding, activation and epoch remain a single tuple.
/// Neither manifest generation nor delta lowering reconstructs authority from
/// projector names, event-name strings, or a union of legacy `.facts(...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceModeledProjection {
    program: ProjectionProgram,
    binding: ProjectionBinding,
    activation: ProjectionBindingActivation,
}

impl SurfaceModeledProjection {
    /// Construct one exact modeled registration.
    ///
    /// Full owner/model/placement validation runs when the declaration is
    /// attached to a catalog Surface, where selected ORM schemas are present.
    pub fn try_new(
        program: ProjectionProgram,
        binding: ProjectionBinding,
        activation: ProjectionBindingActivation,
    ) -> Result<Self, String> {
        let program_id = program.id().map_err(|error| error.to_string())?;
        if binding.program_id() != program_id {
            return Err("modeled projection binding does not match its program digest".into());
        }
        if activation.program_id() != program_id || activation.binding_id() != binding.id() {
            return Err(
                "modeled projection activation does not match its exact program and binding".into(),
            );
        }
        if activation.route().is_none() {
            return Err("modeled projection activation requires an executor route".into());
        }
        Ok(Self {
            program,
            binding,
            activation,
        })
    }

    /// Return the authoritative portable program.
    pub fn program(&self) -> &ProjectionProgram {
        &self.program
    }

    /// Return its semantic program identity.
    pub fn program_id(&self) -> ProjectionProgramId {
        self.binding.program_id()
    }

    /// Return the exact deployment binding.
    pub fn binding(&self) -> &ProjectionBinding {
        &self.binding
    }

    /// Return the exact active or draining executor registration.
    pub fn activation(&self) -> &ProjectionBindingActivation {
        &self.activation
    }

    /// Whether this registration may mint ordinary client causal work.
    pub fn is_causally_eligible(&self) -> bool {
        self.activation.state() == ProjectionBindingState::Active
            && self.binding.placement() == ProjectionPlacement::Eventual
            && self.binding.execution_class() == ProjectionExecutionClass::Causal
    }

    pub(super) fn validate_for_surface(
        &self,
        owner_name: &str,
        kind: SurfaceProjectionOwnerKind,
        models: &std::collections::BTreeMap<String, SurfaceModel>,
    ) -> Result<(), String> {
        if self.binding.owner().name() != owner_name {
            return Err(format!(
                "modeled projection owner `{owner_name}` differs from binding owner `{}`",
                self.binding.owner().name()
            ));
        }
        let expected_placement = match kind {
            SurfaceProjectionOwnerKind::Async => ProjectionPlacement::Eventual,
            SurfaceProjectionOwnerKind::Direct => ProjectionPlacement::Direct,
        };
        if self.binding.placement() != expected_placement {
            return Err(format!(
                "modeled projection owner `{owner_name}` has incompatible {:?} placement",
                self.binding.placement()
            ));
        }
        let output_models = self
            .binding
            .outputs()
            .iter()
            .map(|output| output.model())
            .collect::<BTreeSet<_>>();
        for output in self.binding.outputs() {
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
        for arm in self.program.arms() {
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
            validate_direct_program(owner_name, &self.program, &self.binding, models)?;
        }
        Ok(())
    }
}

fn validate_direct_program(
    owner: &str,
    program: &ProjectionProgram,
    binding: &ProjectionBinding,
    models: &std::collections::BTreeMap<String, SurfaceModel>,
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
