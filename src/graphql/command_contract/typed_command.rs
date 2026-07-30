use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::direct_projection::{
    resolve_trusted_preset, CommandDirectProjectionTarget, CommandProjectedModel,
    CompiledDirectProjectionTarget, DirectProjectionTargetResolutionError,
    ResolvedDirectProjectionTarget,
};
use super::effect_wire::{CompiledCommandEffects, CompiledConfirmationPlan, CompiledInputDefaults};
use super::effects::{
    invalid_confirmation_constant, invalid_expression_constant, CommandEffects, EffectExpression,
};
use super::outcomes::{CommandConsistency, CommandOutcome, Projected};
use super::projection_obligations::{
    CommandInputDefault, CommandProjectionConfirmation, ProjectionObligationResolutionError,
};
use super::projection_proof::{canonical_json, CommandCommitProofError};
use super::projections::{CommandProjectionEvents, CommandProjectionPreview};
use crate::graphql::naming;
use crate::graphql::types::{GraphqlInputType, GraphqlTypeDef};
use crate::microsvc::Session;
use crate::outbox::OutboxMessage;
use crate::projection_protocol::{
    ProjectionEpoch, ProjectionScopeCodec, ResolvedProjectionKey, ResolvedProjectionKeyField,
    ResolvedProjectionObligation,
};
use crate::read_model::RelationalReadModel;

#[derive(Clone, Debug)]
pub(crate) struct TypedCommandContract {
    pub name: String,
    pub field_name: String,
    pub roles: Vec<String>,
    pub input: GraphqlTypeDef,
    pub output: GraphqlTypeDef,
    pub input_type_id: TypeId,
    pub output_type_id: TypeId,
    pub consistency: CommandConsistency,
    pub input_defaults: Vec<CommandInputDefault>,
    pub effects: CommandEffects,
    pub confirmations: Vec<CommandProjectionConfirmation>,
    /// Present automatically for `Projected<M>` before Surface ownership is
    /// resolved. This never requires an application declaration.
    pub projected_model: Option<CommandProjectedModel>,
    pub direct_projection: Option<CommandDirectProjectionTarget>,
    /// Exact outward event contracts, independent of whichever projectors
    /// happen to consume them in this deployment.
    pub projections: CommandProjectionEvents,
}

impl TypedCommandContract {
    /// Stable per-route identity used in the command ledger fingerprint.
    ///
    /// This is intentionally distinct from the service inventory digest: a
    /// deployment may add unrelated routes without invalidating safe retries
    /// for this command. The explicit domain/version prevents this byte digest
    /// from aliasing any other SHA-256 use in the framework.
    pub(crate) fn fingerprint_bytes(&self) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(b"distributed.typed-command-contract.v1");
        digest.update([0]);
        digest.update(
            serde_json::to_vec(&self.canonical_value())
                .expect("canonical typed command contract serialization cannot fail"),
        );
        digest.finalize().into()
    }

    pub(crate) fn canonical_value(&self) -> serde_json::Value {
        let mut roles = self.roles.clone();
        roles.sort();
        roles.dedup();
        let mut effects = self.effects.clone();
        effects.canonicalize();
        let mut input_defaults = self.input_defaults.clone();
        input_defaults.sort_by(|left, right| left.path.cmp(&right.path));
        let mut confirmations = self.confirmations.clone();
        confirmations.sort_by(|left, right| {
            serde_json::to_string(&left.canonical_value())
                .expect("confirmation IR serialization cannot fail")
                .cmp(
                    &serde_json::to_string(&right.canonical_value())
                        .expect("confirmation IR serialization cannot fail"),
                )
        });
        let confirmations = confirmations
            .iter()
            .map(CommandProjectionConfirmation::canonical_value)
            .collect::<Vec<_>>();
        let direct_projection = self
            .direct_projection
            .as_ref()
            .map(CommandDirectProjectionTarget::canonical_value);
        let projected_model = self
            .projected_model
            .as_ref()
            .map(CommandProjectedModel::canonical_value);
        let mut projections = self.projections.clone();
        projections
            .canonicalize_and_validate(&self.name)
            .expect("validated command projection declarations are canonical");
        canonical_json(&serde_json::json!({
            "name": self.name,
            "field_name": self.field_name,
            "roles": roles,
            "input": canonical_graphql_type(&self.input),
            "output": canonical_graphql_type(&self.output),
            "consistency": self.consistency,
            "input_defaults": input_defaults,
            "effects": effects,
            "confirmations": confirmations,
            "projected_model": projected_model,
            "direct_projection": direct_projection,
            "projections": projections,
        }))
    }

    /// Resolve the finite declaration-owned projection plan from the exact
    /// canonical GraphQL wire input retained beside the decoded command.
    ///
    /// Resolution is pure and must run before commit I/O. Confirmation and key
    /// field order are retained exactly as declared. Input values and constants
    /// are cloned without a Rust DTO or SQL codec round trip, while explicit
    /// null remains distinguishable from an undeclared partition.
    #[allow(dead_code)]
    pub(crate) fn resolve_projection_obligations(
        &self,
        canonical_wire_input: &serde_json::Value,
    ) -> Result<Vec<ResolvedProjectionObligation>, ProjectionObligationResolutionError> {
        self.resolve_projection_obligations_from_session(canonical_wire_input, None)
    }

    pub(crate) fn resolve_projection_obligations_from_session(
        &self,
        canonical_wire_input: &serde_json::Value,
        session: Option<&Session>,
    ) -> Result<Vec<ResolvedProjectionObligation>, ProjectionObligationResolutionError> {
        self.confirmations
            .iter()
            .map(|confirmation| {
                let fields = confirmation
                    .key
                    .fields
                    .iter()
                    .map(|field| {
                        let target = format!("key field `{}`", field.field);
                        resolve_projection_obligation_expression(
                            canonical_wire_input,
                            confirmation,
                            &target,
                            &field.value,
                            session,
                            confirmation
                                .schema
                                .and_then(|schema| {
                                    schema
                                        .columns
                                        .iter()
                                        .find(|column| column.column_name == field.field)
                                })
                                .and_then(|column| naming::scalar_type_name(&column.column_type)),
                        )
                        .map(|value| ResolvedProjectionKeyField {
                            field: field.field.clone(),
                            value,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let partition = confirmation
                    .partition
                    .as_ref()
                    .map(|expression| {
                        resolve_projection_obligation_expression(
                            canonical_wire_input,
                            confirmation,
                            "partition",
                            expression,
                            session,
                            Some("String"),
                        )
                    })
                    .transpose()?;
                let key = ResolvedProjectionKey { fields };
                let topology = confirmation.protocol_topology.clone().ok_or_else(|| {
                    ProjectionObligationResolutionError::InvalidBinding {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        reason: "missing compiled topology identity".into(),
                    }
                })?;
                let schema = confirmation.schema.ok_or_else(|| {
                    ProjectionObligationResolutionError::InvalidBinding {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        reason: "missing retained relational schema".into(),
                    }
                })?;
                let codec = ProjectionScopeCodec::with_models(
                    topology,
                    [(schema.model_name.as_str(), schema)],
                )
                .map_err(|error| {
                    ProjectionObligationResolutionError::InvalidBinding {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        reason: error.to_string(),
                    }
                })?;
                let scope = codec
                    .encode_resolved_obligation_scope(
                        &confirmation.projector,
                        &confirmation.model,
                        &key,
                        partition.as_ref(),
                    )
                    .map_err(
                        |error| ProjectionObligationResolutionError::InvalidBinding {
                            projector: confirmation.projector.clone(),
                            model: confirmation.model.clone(),
                            reason: error.to_string(),
                        },
                    )?;
                Ok(ResolvedProjectionObligation {
                    projector: confirmation.projector.clone(),
                    model: confirmation.model.clone(),
                    key,
                    partition,
                    scope,
                })
            })
            .collect()
    }

    pub(crate) fn resolve_direct_projection_target_from_session(
        &self,
        canonical_wire_input: &serde_json::Value,
        session: Option<&Session>,
    ) -> Result<Option<ResolvedDirectProjectionTarget>, DirectProjectionTargetResolutionError> {
        self.direct_projection
            .as_ref()
            .map(|target| target.resolve(canonical_wire_input, session))
            .transpose()
    }

    /// Prove that every finite asynchronous confirmation can be driven by a
    /// fact staged in the durable outbox. Aggregate event records intentionally
    /// do not count: they are write-side history and have no publication path.
    pub(super) fn validate_outbox_fact_coverage(
        &self,
        outbox_messages: &[OutboxMessage],
    ) -> Result<(), CommandCommitProofError> {
        if self.consistency == CommandConsistency::Causal
            && self.confirmations.is_empty()
            && self.projections.selectors.is_empty()
        {
            return Err(CommandCommitProofError::CausalHasNoConfirmations);
        }

        let staged_facts = outbox_messages
            .iter()
            .filter(|message| message.destination.is_none() && message.is_pending())
            .map(|message| message.event_type.clone())
            .collect::<BTreeSet<_>>();
        for confirmation in &self.confirmations {
            if confirmation
                .projector_topology
                .facts
                .iter()
                .any(|fact| staged_facts.contains(fact))
            {
                continue;
            }
            return Err(CommandCommitProofError::UnreachableConfirmation {
                projector: confirmation.projector.clone(),
                expected_facts: confirmation.projector_topology.facts.clone(),
                staged_facts: staged_facts.iter().cloned().collect(),
            });
        }
        Ok(())
    }
}

fn resolve_projection_obligation_expression(
    canonical_wire_input: &serde_json::Value,
    confirmation: &CommandProjectionConfirmation,
    target: &str,
    expression: &EffectExpression,
    session: Option<&Session>,
    expected_scalar: Option<&str>,
) -> Result<serde_json::Value, ProjectionObligationResolutionError> {
    match expression {
        EffectExpression::Input { path } => {
            let mut value = canonical_wire_input;
            if path.is_empty() {
                return Err(ProjectionObligationResolutionError::MissingInputPath {
                    projector: confirmation.projector.clone(),
                    model: confirmation.model.clone(),
                    target: target.to_string(),
                    path: path.clone(),
                });
            }
            for segment in path {
                let Some(next) = value.as_object().and_then(|object| object.get(segment)) else {
                    return Err(ProjectionObligationResolutionError::MissingInputPath {
                        projector: confirmation.projector.clone(),
                        model: confirmation.model.clone(),
                        target: target.to_string(),
                        path: path.clone(),
                    });
                };
                value = next;
            }
            Ok(value.clone())
        }
        EffectExpression::Constant { value } => Ok(value.clone()),
        EffectExpression::Null => Ok(serde_json::Value::Null),
        EffectExpression::TrustedPreset { name } => {
            resolve_trusted_preset(session, name, expected_scalar.unwrap_or("String")).ok_or_else(
                || ProjectionObligationResolutionError::TrustedPresetUnavailable {
                    projector: confirmation.projector.clone(),
                    model: confirmation.model.clone(),
                    target: target.to_string(),
                    preset: name.clone(),
                },
            )
        }
        EffectExpression::InvalidConstant { error } => {
            Err(ProjectionObligationResolutionError::InvalidConstant {
                projector: confirmation.projector.clone(),
                model: confirmation.model.clone(),
                target: target.to_string(),
                error: error.clone(),
            })
        }
    }
}

fn canonical_graphql_type(definition: &GraphqlTypeDef) -> serde_json::Value {
    let mut fields = definition.fields.iter().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.name.cmp(&right.name));
    serde_json::json!({
        "name": definition.name,
        "fields": fields.into_iter().map(|field| serde_json::json!({
            "name": field.name,
            "type_name": field.type_name,
            "nullable": field.nullable,
            "list": field.list,
            "item_nullable": field.item_nullable,
            "nested": field.nested.as_deref().map(canonical_graphql_type),
        })).collect::<Vec<_>>(),
    })
}

/// Stable command inventory identity shared by a service and GraphQL engine.
///
/// The digest covers canonical wire structure while the non-serializable
/// `TypeId` pairs prove that both sides were built from the exact Rust input
/// and output types, not merely lookalike GraphQL shapes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TypedServiceCommandBinding {
    pub service_id: String,
    pub structural_fingerprint: String,
    pub types: BTreeMap<String, (TypeId, TypeId)>,
}

impl TypedServiceCommandBinding {
    pub(crate) fn from_contracts(
        service_id: &str,
        contracts: &[TypedCommandContract],
    ) -> Result<Self, String> {
        if service_id.trim().is_empty() {
            return Err("typed command inventory requires a non-empty service ID".into());
        }

        let mut seen = BTreeSet::new();
        let mut ordered = contracts.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| left.name.cmp(&right.name));
        let mut types = BTreeMap::new();
        let mut canonical = Vec::with_capacity(ordered.len());
        for contract in ordered {
            let mut projections = contract.projections.clone();
            projections.canonicalize_and_validate(&contract.name)?;
            if contract.name.trim().is_empty() {
                return Err("typed command id must not be empty".into());
            }
            if !seen.insert(contract.name.clone()) {
                return Err(format!(
                    "duplicate typed command declaration for `{}`",
                    contract.name
                ));
            }
            if contract.input.type_id != Some(contract.input_type_id) {
                return Err(format!(
                    "typed command `{}` input GraphQL metadata is missing or has a different Rust TypeId",
                    contract.name
                ));
            }
            if contract.output.type_id != Some(contract.output_type_id) {
                return Err(format!(
                    "typed command `{}` output GraphQL metadata is missing or has a different Rust TypeId",
                    contract.name
                ));
            }
            match contract.consistency {
                CommandConsistency::Causal
                    if contract.confirmations.is_empty()
                        && contract.projections.selectors.is_empty() =>
                {
                    return Err(format!(
                        "typed causal command `{}` must declare at least one expected projector confirmation",
                        contract.name
                    ));
                }
                CommandConsistency::Projected if !contract.confirmations.is_empty() => {
                    return Err(format!(
                        "typed projected command `{}` cannot declare asynchronous projector confirmations",
                        contract.name
                    ));
                }
                CommandConsistency::Projected if contract.projected_model.is_none() => {
                    return Err(format!(
                        "typed projected command `{}` is missing its compiler-retained relational model",
                        contract.name
                    ));
                }
                CommandConsistency::Succeeded | CommandConsistency::Causal
                    if contract.projected_model.is_some()
                        || contract.direct_projection.is_some() =>
                {
                    return Err(format!(
                        "typed non-projected command `{}` cannot carry direct projection metadata",
                        contract.name
                    ));
                }
                CommandConsistency::Causal
                | CommandConsistency::Succeeded
                | CommandConsistency::Projected => {}
            }
            if let Some(projected) = &contract.projected_model {
                if projected.output_type_id != contract.output_type_id {
                    return Err(format!(
                        "typed projected command `{}` retained model has a different Rust output type",
                        contract.name
                    ));
                }
                if projected.model != contract.output.name {
                    return Err(format!(
                        "typed projected command `{}` retained model `{}` differs from output `{}`",
                        contract.name, projected.model, contract.output.name
                    ));
                }
            }
            if let Some(target) = &contract.direct_projection {
                if target.output_type_id != contract.output_type_id {
                    return Err(format!(
                        "typed projected command `{}` direct target has a different Rust output type",
                        contract.name
                    ));
                }
                if target.model != contract.output.name {
                    return Err(format!(
                        "typed projected command `{}` direct target model `{}` differs from output `{}`",
                        contract.name, target.model, contract.output.name
                    ));
                }
                let Some(change_epoch) = target.change_epoch.as_deref() else {
                    return Err(format!(
                        "typed projected command `{}` direct target has no registered change-log epoch",
                        contract.name
                    ));
                };
                ProjectionEpoch::new(change_epoch).map_err(|error| {
                    format!(
                        "typed projected command `{}` direct target change epoch is invalid: {error}",
                        contract.name
                    )
                })?;
            }
            if let Some(error) = contract
                .effects
                .invalid_constant_error()
                .or_else(|| {
                    contract
                        .confirmations
                        .iter()
                        .find_map(invalid_confirmation_constant)
                })
                .or_else(|| {
                    contract
                        .direct_projection
                        .as_ref()
                        .and_then(|target| target.partition.as_ref())
                        .and_then(invalid_expression_constant)
                })
                .or_else(|| {
                    contract
                        .projected_model
                        .as_ref()
                        .and_then(|model| model.partition.as_ref())
                        .and_then(invalid_expression_constant)
                })
            {
                return Err(format!(
                    "typed command `{}` constant effect value failed to serialize: {error}",
                    contract.name
                ));
            }
            let mut confirmations = BTreeSet::new();
            for confirmation in &contract.confirmations {
                let canonical = serde_json::to_string(&confirmation.canonical_value())
                    .expect("confirmation IR serialization cannot fail");
                if !confirmations.insert(canonical) {
                    return Err(format!(
                        "typed command `{}` repeats an expected projector confirmation",
                        contract.name
                    ));
                }
            }
            types.insert(
                contract.name.clone(),
                (contract.input_type_id, contract.output_type_id),
            );
            canonical.push(contract.canonical_value());
        }

        let material = canonical_json(&serde_json::json!({
            "service_id": service_id,
            "commands": canonical,
        }));
        let bytes = serde_json::to_vec(&material)
            .expect("serializing canonical command inventory cannot fail");
        Ok(Self {
            service_id: service_id.to_string(),
            structural_fingerprint: format!("sha256:{:x}", Sha256::digest(bytes)),
            types,
        })
    }
}

/// A typed command declaration registered together with its executable handler.
pub struct TypedCommand<I, K: CommandOutcome> {
    route_name: &'static str,
    contract: TypedCommandContract,
    _types: PhantomData<fn(I) -> K>,
}

impl<I, K: CommandOutcome> Clone for TypedCommand<I, K> {
    fn clone(&self) -> Self {
        Self {
            route_name: self.route_name,
            contract: self.contract.clone(),
            _types: PhantomData,
        }
    }
}

/// Begin a typed command declaration.
pub fn typed_command<I, K>(name: &'static str) -> TypedCommand<I, K>
where
    I: GraphqlInputType + DeserializeOwned + Send + 'static,
    K: CommandOutcome,
{
    let route_name = name;
    let name = route_name.to_string();
    let field_name = name
        .chars()
        .map(|character| match character {
            '.' | '-' => '_',
            other => other,
        })
        .collect();
    let input = I::graphql_type();
    let output = K::__graphql_output_type();
    let projected_model = K::__projected_model()
        .map(|(output_type_id, schema)| CommandProjectedModel::new(output_type_id, schema));
    TypedCommand {
        route_name,
        contract: TypedCommandContract {
            name,
            field_name,
            roles: Vec::new(),
            input,
            output,
            input_type_id: TypeId::of::<I>(),
            output_type_id: TypeId::of::<K::Payload>(),
            consistency: K::CONSISTENCY,
            input_defaults: Vec::new(),
            effects: CommandEffects::revalidate(),
            confirmations: Vec::new(),
            projected_model,
            direct_projection: None,
            projections: CommandProjectionEvents::default(),
        },
        _types: PhantomData,
    }
}

impl<I, K: CommandOutcome> TypedCommand<I, K> {
    pub fn field_name(mut self, field_name: impl Into<String>) -> Self {
        self.contract.field_name = field_name.into();
        self
    }

    pub fn roles(mut self, roles: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.contract.roles = roles.into_iter().map(Into::into).collect();
        self.contract.roles.sort();
        self.contract.roles.dedup();
        self
    }

    pub fn effects(mut self, effects: CompiledCommandEffects<I>) -> Self {
        self.contract.effects = effects.0;
        self.contract.effects.canonicalize();
        self
    }

    /// Declare values generated once into the canonical command input before
    /// dispatch. Effects and confirmations must reference the finalized input
    /// field rather than invoking a generator independently.
    pub fn input_defaults(mut self, defaults: CompiledInputDefaults<I>) -> Self {
        self.contract.input_defaults = defaults.0;
        self.contract
            .input_defaults
            .sort_by(|left, right| left.path.cmp(&right.path));
        self
    }

    /// Declare the finite projector/model/key progress that confirms this
    /// causal outcome.
    /// Legacy `Causal<_>` commands require at least one confirmation unless
    /// they declare an exact emitted-event set for modeled runtime derivation.
    /// `Succeeded<_>` may omit the plan (terminal succeeded) or provide one.
    /// `Projected<_>` commands cannot carry asynchronous confirmations.
    pub fn confirmations(mut self, confirmations: CompiledConfirmationPlan<I>) -> Self {
        self.contract.confirmations = confirmations.0;
        self
    }

    /// Declare an exact outward domain-event set this command may emit.
    ///
    /// This declaration is intentionally independent of projector ownership:
    /// one occurrence can fan out to zero, one, or many modeled programs.
    #[must_use]
    pub fn emits(mut self, events: super::CommandProjectionEventSet) -> Self {
        self.contract.projections.add_event_set(events);
        self
    }

    /// Predict one ordered, non-authoritative occurrence for the optimistic
    /// client overlay.
    ///
    /// Service registration rejects a preview whose exact event selector is
    /// not also present through [`Self::emits`]. Repeating an exact selector
    /// predicts multiple occurrences in declaration order; it does not promise
    /// that the server emits any particular cardinality. The authoritative
    /// ordered command delta later replaces the optimistic overlay.
    #[must_use]
    pub fn preview(mut self, preview: CommandProjectionPreview) -> Self {
        self.contract.projections.add_preview(preview);
        self
    }

    pub fn name(&self) -> &str {
        &self.contract.name
    }

    pub fn consistency(&self) -> CommandConsistency {
        self.contract.consistency
    }

    #[cfg(test)]
    pub(crate) fn into_contract(self) -> TypedCommandContract {
        self.contract
    }

    pub(crate) fn into_parts(self) -> (&'static str, TypedCommandContract) {
        (self.route_name, self.contract)
    }
}

impl<I, M> TypedCommand<I, Projected<M>>
where
    I: GraphqlInputType + DeserializeOwned + Send + 'static,
    M: RelationalReadModel + Serialize + Send + Sync + 'static,
{
    /// Attach compiler-generated direct projection ownership metadata.
    ///
    /// Application handlers do not call this; generated service inventory
    /// binds it from the registered projector declaration and handlers use the
    /// fluent direct-projection commit API.
    #[doc(hidden)]
    pub fn __direct_projection(mut self, target: CompiledDirectProjectionTarget<I, M>) -> Self {
        if let Some(projected) = &mut self.contract.projected_model {
            projected.partition = target.0.partition.clone();
        }
        self.contract.direct_projection = Some(target.0);
        self
    }
}
