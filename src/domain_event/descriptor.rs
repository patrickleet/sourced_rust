use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::{DOMAIN_EVENT_BODY_CODEC, DOMAIN_EVENT_BODY_CODEC_VERSION};

/// How a public domain-event body was derived.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainEventBodyKind {
    /// A deliberately public post-transition aggregate state.
    State,
    /// A sparse or explicitly adapted outward event.
    Event,
    /// A stable identity and incarnation for physical deletion.
    Deletion,
}

/// Versioned schema and codec identity for one domain-event body.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DomainEventBodyDescriptor {
    /// Whether the body is state, a sparse event, or deletion identity.
    pub kind: DomainEventBodyKind,
    /// Stable body type name used by generated schemas.
    pub type_name: Cow<'static, str>,
    /// Independently evolving body schema version.
    pub version: u64,
    /// Canonical schema identifier generated for this body.
    pub schema: Cow<'static, str>,
    /// Lowercase `sha256:` fingerprint of the canonical body schema.
    pub fingerprint: Cow<'static, str>,
    /// Canonical body codec.
    pub codec: Cow<'static, str>,
    /// Canonical body codec version.
    pub codec_version: u16,
}

impl DomainEventBodyDescriptor {
    /// Declare a version-one Distributed JSON body descriptor.
    pub const fn distributed_json(
        kind: DomainEventBodyKind,
        type_name: &'static str,
        version: u64,
        schema: &'static str,
        fingerprint: &'static str,
    ) -> Self {
        Self {
            kind,
            type_name: Cow::Borrowed(type_name),
            version,
            schema: Cow::Borrowed(schema),
            fingerprint: Cow::Borrowed(fingerprint),
            codec: Cow::Borrowed(DOMAIN_EVENT_BODY_CODEC),
            codec_version: DOMAIN_EVENT_BODY_CODEC_VERSION,
        }
    }
}

/// Independently versioned public post-transition state descriptor.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DomainStateDescriptor {
    /// Stable state type name used by generated schemas.
    pub type_name: Cow<'static, str>,
    /// Public state schema version.
    pub version: u64,
    /// Canonical public state schema identifier.
    pub schema: Cow<'static, str>,
    /// Lowercase `sha256:` fingerprint of the public state schema.
    pub fingerprint: Cow<'static, str>,
    /// Canonical public state codec.
    pub codec: Cow<'static, str>,
    /// Canonical public state codec version.
    pub codec_version: u16,
}

impl DomainStateDescriptor {
    /// Declare a version-one Distributed JSON domain-state descriptor.
    pub const fn distributed_json(
        type_name: &'static str,
        version: u64,
        schema: &'static str,
        fingerprint: &'static str,
    ) -> Self {
        Self {
            type_name: Cow::Borrowed(type_name),
            version,
            schema: Cow::Borrowed(schema),
            fingerprint: Cow::Borrowed(fingerprint),
            codec: Cow::Borrowed(DOMAIN_EVENT_BODY_CODEC),
            codec_version: DOMAIN_EVENT_BODY_CODEC_VERSION,
        }
    }

    /// Use this state schema as the body of a semantic domain event.
    pub fn event(self, name: impl Into<Cow<'static, str>>, version: u64) -> DomainEventDescriptor {
        DomainEventDescriptor {
            name: name.into(),
            version,
            body: self.into(),
        }
    }
}

impl From<DomainStateDescriptor> for DomainEventBodyDescriptor {
    fn from(state: DomainStateDescriptor) -> Self {
        Self {
            kind: DomainEventBodyKind::State,
            type_name: state.type_name,
            version: state.version,
            schema: state.schema,
            fingerprint: state.fingerprint,
            codec: state.codec,
            codec_version: state.codec_version,
        }
    }
}

/// Semantic domain-event name and independently versioned body contract.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DomainEventDescriptor {
    /// Stable semantic event name, such as `todo.completed`.
    pub name: Cow<'static, str>,
    /// Semantic event version, independent of its body schema version.
    pub version: u64,
    /// Typed body schema and codec.
    pub body: DomainEventBodyDescriptor,
}

impl DomainEventDescriptor {
    /// Construct a descriptor for a state-capture event.
    pub fn state<S: DomainState>(name: impl Into<Cow<'static, str>>, version: u64) -> Self {
        S::DESCRIPTOR.clone().event(name, version)
    }
}

/// A deliberately public post-transition state DTO.
///
/// This is separate from `Snapshot`: snapshot fields and codecs are private
/// replay-acceleration details, while this descriptor is a durable public
/// contract.
pub trait DomainState: Serialize {
    /// Schema identity of the public state body.
    const DESCRIPTOR: DomainStateDescriptor;
}

/// A sparse or explicitly adapted outward domain event.
pub trait DomainEvent: Serialize {
    /// Semantic event and body contract.
    const DESCRIPTOR: DomainEventDescriptor;
}

/// Exact typed contract for one outward event a command may declare.
///
/// Unlike [`DomainEvent`], the contract type does not have to be the value
/// serialized on the wire. Sourced state and deletion transitions therefore
/// use uninhabited marker types whose `Body` is the actual state/deletion DTO.
#[doc(hidden)]
pub trait DomainEventContract {
    /// Exact serialized outward body.
    type Body: Serialize;

    /// Stable semantic event name.
    const EVENT_NAME: &'static str;

    /// Independently versioned semantic event version.
    const EVENT_VERSION: u64;

    /// Exact semantic event and body descriptor.
    fn descriptor() -> DomainEventDescriptor;
}

/// Stable deletion body used when no live post-state exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainDeletion<K> {
    /// Logical aggregate/read-model key being deleted.
    pub key: K,
    /// Non-zero incarnation being deleted.
    pub incarnation: u64,
}

impl<K> DomainDeletion<K> {
    /// Construct a deletion body for a known live incarnation.
    ///
    /// # Errors
    ///
    /// Returns [`DomainDeletionError`] when `incarnation` is zero.
    pub fn new(key: K, incarnation: u64) -> Result<Self, DomainDeletionError> {
        if incarnation == 0 {
            return Err(DomainDeletionError);
        }
        Ok(Self { key, incarnation })
    }
}

/// A deletion incarnation must be non-zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DomainDeletionError;

impl fmt::Display for DomainDeletionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("domain deletion incarnation must be non-zero")
    }
}

impl std::error::Error for DomainDeletionError {}
