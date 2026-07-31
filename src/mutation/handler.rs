//! Portable and custom projection event handlers over mutation programs.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::projection::{
    ProjectionEventSelector, ProjectionExpression, ProjectionPartition, ProjectionProgramId,
    ProjectionValueType,
};

use super::bind::{MutationEventBinding, MutationInputBinding};
use super::canonical::canonical_json_bytes;
use super::program::{MutationProgram, MutationProgramId};
use super::MutationProgramError;

/// Placement for a projector handler registration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationHandlerPlacement {
    /// Asynchronous local projector.
    EventualLocal,
    /// Asynchronous remote projector.
    EventualRemote,
    /// Same-transaction direct projector.
    Direct,
}

/// Ownership and topology metadata for one portable handler.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationHandlerRegistration {
    /// Stable projector owner name.
    owner: String,
    /// Ownership / rebuild epoch.
    epoch: String,
    /// Placement class.
    placement: MutationHandlerPlacement,
    /// Logical partition expression encoded as projection partition.
    #[serde(skip)]
    partition: ProjectionPartition,
    /// Event-to-mutation binding.
    #[serde(skip)]
    binding: MutationEventBinding,
    /// Optional human-readable handler name.
    name: String,
    /// Independently evolving handler version.
    version: u64,
}

impl MutationHandlerRegistration {
    /// Construct a portable handler registration.
    ///
    /// # Errors
    ///
    /// Rejects empty names.
    pub fn try_new(
        name: impl Into<String>,
        version: u64,
        owner: impl Into<String>,
        epoch: impl Into<String>,
        placement: MutationHandlerPlacement,
        partition: ProjectionPartition,
        binding: MutationEventBinding,
    ) -> Result<Self, MutationProgramError> {
        let name = super::expression::non_empty(name.into(), "handler name")?;
        let owner = super::expression::non_empty(owner.into(), "handler owner")?;
        let epoch = super::expression::non_empty(epoch.into(), "handler epoch")?;
        if version == 0 {
            return Err(MutationProgramError::ZeroVersion("handler version"));
        }
        Ok(Self {
            owner,
            epoch,
            placement,
            partition,
            binding,
            name,
            version,
        })
    }

    /// Return the handler name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the handler version.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Return the owner name.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Return the ownership epoch.
    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    /// Return placement.
    pub fn placement(&self) -> MutationHandlerPlacement {
        self.placement
    }

    /// Return partition.
    pub fn partition(&self) -> &ProjectionPartition {
        &self.partition
    }

    /// Return the event-to-mutation binding.
    pub fn binding(&self) -> &MutationEventBinding {
        &self.binding
    }

    /// Return the uniqueness key `(owner, event contract, epoch)`.
    pub fn uniqueness_key(&self) -> MutationHandlerUniquenessKey {
        MutationHandlerUniquenessKey {
            owner: self.owner.clone(),
            event_name: self.binding.selector().event_name().to_owned(),
            event_version: self.binding.selector().event_version(),
            body_fingerprint: self.binding.selector().body_fingerprint().to_owned(),
            epoch: self.epoch.clone(),
        }
    }

    /// Derive target models from the bound mutation program.
    pub fn target_models(&self) -> Vec<String> {
        let mut models = self
            .binding
            .program()
            .operations()
            .iter()
            .map(|operation| operation.target().model().to_owned())
            .collect::<Vec<_>>();
        models.sort();
        models.dedup();
        models
    }

    /// Stable digest of owner, event, epoch, placement, mutation program id.
    ///
    /// # Errors
    ///
    /// Propagates canonical encoding failures.
    pub fn digest(&self) -> Result<[u8; 32], MutationProgramError> {
        #[derive(Serialize)]
        struct DigestBody<'a> {
            name: &'a str,
            version: u64,
            owner: &'a str,
            epoch: &'a str,
            placement: MutationHandlerPlacement,
            mutation_program_id: String,
            selector_event: &'a str,
            selector_version: u64,
            selector_fingerprint: &'a str,
        }
        let program_id = self.binding.program().id()?;
        let body = DigestBody {
            name: &self.name,
            version: self.version,
            owner: &self.owner,
            epoch: &self.epoch,
            placement: self.placement,
            mutation_program_id: program_id.to_string(),
            selector_event: self.binding.selector().event_name(),
            selector_version: self.binding.selector().event_version(),
            selector_fingerprint: self.binding.selector().body_fingerprint(),
        };
        let bytes = canonical_json_bytes(&body)?;
        let mut digest = Sha256::new();
        digest.update(b"distributed.mutation-handler/v1\0");
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(&bytes);
        Ok(digest.finalize().into())
    }

    /// Materialize the internal projection program for the existing runtime.
    ///
    /// # Errors
    ///
    /// Propagates rewrite failures.
    pub fn to_projection_program(
        &self,
    ) -> Result<crate::projection::ProjectionProgram, MutationProgramError> {
        self.binding.to_projection_program(
            self.name.clone(),
            self.version,
            self.partition.clone(),
            format!("{}-arm", self.name),
        )
    }
}

/// Uniqueness key for portable handler bindings.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct MutationHandlerUniquenessKey {
    /// Projector owner.
    pub owner: String,
    /// Semantic event name.
    pub event_name: String,
    /// Semantic event version.
    pub event_version: u64,
    /// Body fingerprint.
    pub body_fingerprint: String,
    /// Ownership epoch.
    pub epoch: String,
}

/// Deployment catalog of portable mutation handlers.
#[derive(Clone, Debug, Default)]
pub struct MutationHandlerCatalog {
    registrations: Vec<MutationHandlerRegistration>,
}

impl MutationHandlerCatalog {
    /// Construct an empty catalog.
    pub fn new() -> Self {
        Self {
            registrations: Vec::new(),
        }
    }

    /// Register a portable handler, rejecting uniqueness and dual-writer conflicts.
    ///
    /// # Errors
    ///
    /// Rejects duplicate `(owner, event, epoch)` bindings and dual writers for
    /// the same model/partition/epoch with incompatible placement.
    pub fn register(
        &mut self,
        registration: MutationHandlerRegistration,
    ) -> Result<(), MutationProgramError> {
        let key = registration.uniqueness_key();
        if self
            .registrations
            .iter()
            .any(|existing| existing.uniqueness_key() == key)
        {
            return Err(MutationProgramError::InvalidOperation {
                operation: registration.name().to_owned(),
                reason: format!(
                    "duplicate binding for owner `{}` event `{}` v{} epoch `{}`",
                    key.owner, key.event_name, key.event_version, key.epoch
                ),
            });
        }
        // Dual-writer check: same model + epoch + overlapping placement classes.
        for model in registration.target_models() {
            for existing in &self.registrations {
                if existing.epoch() != registration.epoch() {
                    continue;
                }
                if !existing.target_models().iter().any(|item| item == &model) {
                    continue;
                }
                let direct_overlap = matches!(
                    (existing.placement(), registration.placement()),
                    (
                        MutationHandlerPlacement::Direct,
                        MutationHandlerPlacement::Direct
                    ) | (
                        MutationHandlerPlacement::Direct,
                        MutationHandlerPlacement::EventualLocal
                            | MutationHandlerPlacement::EventualRemote
                    ) | (
                        MutationHandlerPlacement::EventualLocal
                            | MutationHandlerPlacement::EventualRemote,
                        MutationHandlerPlacement::Direct
                    )
                );
                // Same model+epoch with any two writers is rejected in v1 when
                // either is direct or both are eventual for the same partition unit.
                if direct_overlap
                    || (existing.owner() != registration.owner()
                        && existing.placement() == registration.placement())
                {
                    return Err(MutationProgramError::InvalidOperation {
                        operation: registration.name().to_owned(),
                        reason: format!(
                            "dual writer for model `{model}` epoch `{}` between `{}` and `{}`",
                            registration.epoch(),
                            existing.name(),
                            registration.name()
                        ),
                    });
                }
            }
        }
        self.registrations.push(registration);
        Ok(())
    }

    /// Return all registrations.
    pub fn registrations(&self) -> &[MutationHandlerRegistration] {
        &self.registrations
    }

    /// Find registrations for a given event selector.
    pub fn for_selector(
        &self,
        selector: &ProjectionEventSelector,
    ) -> Vec<&MutationHandlerRegistration> {
        self.registrations
            .iter()
            .filter(|registration| registration.binding().selector() == selector)
            .collect()
    }
}

/// Descriptor for a custom (non-portable) async handler that emits mutations.
#[derive(Clone, Debug)]
pub struct CustomMutationHandler {
    /// Handler name.
    pub name: String,
    /// Owner name.
    pub owner: String,
    /// Epoch.
    pub epoch: String,
    /// Placement.
    pub placement: MutationHandlerPlacement,
    /// Event selector.
    pub selector: ProjectionEventSelector,
    /// Mutations this custom handler is allowed to emit.
    pub allowed_programs: Vec<MutationProgramId>,
}

impl CustomMutationHandler {
    /// Construct a custom handler descriptor.
    pub fn new(
        name: impl Into<String>,
        owner: impl Into<String>,
        epoch: impl Into<String>,
        placement: MutationHandlerPlacement,
        selector: ProjectionEventSelector,
        allowed_programs: Vec<MutationProgramId>,
    ) -> Self {
        Self {
            name: name.into(),
            owner: owner.into(),
            epoch: epoch.into(),
            placement,
            selector,
            allowed_programs,
        }
    }

    /// Custom handlers are never portable to the browser.
    pub fn is_portable(&self) -> bool {
        false
    }
}

/// Compose a portable binding from event field paths into mutation inputs.
pub fn portable_binding(
    selector: ProjectionEventSelector,
    program: MutationProgram,
    field_pairs: &[(&[&str], &[&str], ProjectionValueType)],
) -> Result<MutationEventBinding, MutationProgramError> {
    let inputs = field_pairs
        .iter()
        .map(|(input, body, value_type)| {
            super::bind::body_field_binding(
                input.iter().copied(),
                body.iter().copied(),
                value_type.clone(),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    MutationEventBinding::try_new(selector, inputs, program)
}

/// Helper to build an input binding list from explicit expressions.
pub fn bindings_from_expressions(
    pairs: Vec<(Vec<String>, ProjectionExpression)>,
) -> Result<Vec<MutationInputBinding>, MutationProgramError> {
    pairs
        .into_iter()
        .map(|(path, expression)| MutationInputBinding::try_new(path, expression))
        .collect()
}

// Keep ProjectionProgramId import used for documentation symmetry.
#[allow(dead_code)]
fn _projection_program_id_type(_: ProjectionProgramId) {}
