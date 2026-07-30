//! Preview composition: command preview → binding → mutation → cache effects.

use crate::projection::{ProjectionEventSelector, ProjectionExpression, ProjectionValueType};
use crate::DomainEventOccurrence;

use super::bind::MutationEventBinding;
use super::cache::{lower_mutation_cache, MutationCacheProgram, MutationCacheVisibility};
use super::handler::MutationHandlerRegistration;
use super::program::MutationProgram;
use super::server::MutationServerInterpreter;
use super::MutationProgramError;

/// One role-visible owner contribution to a command optimistic layer.
#[derive(Clone, Debug)]
pub struct PreviewOwnerContribution {
    /// Owner name.
    pub owner: String,
    /// Mutation program applied.
    pub program: MutationProgram,
    /// Cache effects for the owner program.
    pub cache: MutationCacheProgram,
}

/// Composed optimistic layer for one predicted event occurrence.
#[derive(Clone, Debug, Default)]
pub struct ComposedPreviewLayer {
    /// Per-owner contributions in stable owner order.
    pub contributions: Vec<PreviewOwnerContribution>,
}

impl ComposedPreviewLayer {
    /// Return whether any portable binding contributed optimism.
    pub fn has_optimism(&self) -> bool {
        !self.contributions.is_empty()
    }

    /// Flatten all cache effects across owners (still applied as one layer).
    pub fn all_effects(&self) -> Vec<&super::cache::MutationCacheEffect> {
        self.contributions
            .iter()
            .flat_map(|contribution| contribution.cache.effects())
            .collect()
    }
}

/// Compose preview → portable bindings → mutations → cache for one predicted event.
///
/// Zero applicable bindings yields an empty layer (scoped revalidation, no invented optimism).
/// Several owners may contribute; more than one binding per owner is rejected by the catalog.
///
/// # Errors
///
/// Propagates cache lowering failures.
pub fn compose_event_preview(
    handlers: &[&MutationHandlerRegistration],
    selector: &ProjectionEventSelector,
    visibility: &MutationCacheVisibility,
) -> Result<ComposedPreviewLayer, MutationProgramError> {
    let mut matching = handlers
        .iter()
        .filter(|handler| handler.binding().selector() == selector)
        .copied()
        .collect::<Vec<_>>();
    matching.sort_by_key(|handler| handler.owner().to_owned());
    let mut contributions = Vec::with_capacity(matching.len());
    for handler in matching {
        let cache = lower_mutation_cache(handler.binding().program(), visibility)?;
        contributions.push(PreviewOwnerContribution {
            owner: handler.owner().to_owned(),
            program: handler.binding().program().clone(),
            cache,
        });
    }
    Ok(ComposedPreviewLayer { contributions })
}

/// Replace a preview layer with actual-event mutation results.
///
/// # Errors
///
/// Propagates server resolve / rewrite failures for the actual occurrence.
pub fn reconcile_with_actual(
    interpreter: &MutationServerInterpreter,
    occurrence: &DomainEventOccurrence,
    visibility: &MutationCacheVisibility,
) -> Result<MutationCacheProgram, MutationProgramError> {
    // Ensure the occurrence matches and can resolve through mutation IR.
    let _resolved = interpreter.resolve(occurrence)?;
    lower_mutation_cache(interpreter.program(), visibility)
}

/// Causal obligation scope derived from mutation targets.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MutationCausalScope {
    /// Logical model name.
    pub model: String,
    /// Opaque storage identity.
    pub storage: String,
}

/// Collect deduplicated causal obligation scopes from composed contributions.
pub fn causal_scopes(layer: &ComposedPreviewLayer) -> Vec<MutationCausalScope> {
    let mut scopes = layer
        .contributions
        .iter()
        .flat_map(|contribution| {
            contribution
                .program
                .operations()
                .iter()
                .map(|operation| MutationCausalScope {
                    model: operation.target().model().to_owned(),
                    storage: operation.target().storage().to_owned(),
                })
        })
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes
}

/// Prove zero-binding behavior: empty handlers produce no optimism.
pub fn zero_binding_preview() -> ComposedPreviewLayer {
    ComposedPreviewLayer::default()
}

/// Helper used by tests to rewrite a binding with an explicit input binder.
pub fn rewrite_binding_ops(
    binding: &MutationEventBinding,
    bind_input_path: &dyn Fn(
        &[String],
        &ProjectionValueType,
    ) -> Result<ProjectionExpression, MutationProgramError>,
) -> Result<Vec<crate::projection::ProjectionOperation>, MutationProgramError> {
    binding
        .program()
        .rewrite_to_projection_operations(bind_input_path)
}
