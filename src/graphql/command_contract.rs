//! Typed command consistency, prepared completions, and portable client effects.
//!
//! This module deliberately separates declaration from durable completion. A
//! handler may prepare a typed payload, but it cannot choose which projection
//! confirmations count. That finite plan belongs to the command declaration,
//! and only the command-ledger committer (task 5) may turn a preparation into
//! an [`Accepted`], [`Fact`], or [`Projected`] value.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef};
use crate::read_model::RelationalReadModel;

/// The consistency guarantee declared by a typed command handler.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandConsistency {
    /// The command was accepted. With no confirmation plan this is terminal;
    /// with an explicit finite plan it is accepted pending projection.
    Accepted,
    /// A durable fact was committed and declared projectors are expected.
    Fact,
    /// The returned view was committed in the command transaction.
    Projected,
}

mod sealed {
    pub trait Outcome {}
    pub trait PreparableOutcome {}
}

/// A committed accepted command result.
///
/// There is intentionally no public constructor. Task 5's durable command
/// committer is the only framework component allowed to create this wrapper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Accepted<T> {
    payload: T,
}

/// A committed durable-fact command result.
///
/// There is intentionally no public constructor. Task 5's durable command
/// committer is the only framework component allowed to create this wrapper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fact<T> {
    payload: T,
}

/// A committed same-transaction projection result.
///
/// There is intentionally no public constructor. Task 5's durable command
/// committer is the only framework component allowed to create this wrapper.
/// Generic declarations remain non-constructible until task 5 adds the
/// `RelationalReadModel` bound and staged transactional projection proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projected<T> {
    payload: T,
}

macro_rules! committed_outcome {
    ($wrapper:ident, $kind:expr) => {
        impl<T> sealed::Outcome for $wrapper<T> {}

        impl<T> CommandOutcome for $wrapper<T>
        where
            T: GraphqlOutputType + Serialize + Send + Sync + 'static,
        {
            type Payload = T;
            const CONSISTENCY: CommandConsistency = $kind;

            fn payload(&self) -> &T {
                &self.payload
            }
        }
    };
}

committed_outcome!(Accepted, CommandConsistency::Accepted);
committed_outcome!(Fact, CommandConsistency::Fact);
committed_outcome!(Projected, CommandConsistency::Projected);

macro_rules! crate_committed_constructor {
    ($wrapper:ident) => {
        impl<T> $wrapper<T> {
            /// Task 5's ledger-aware committer is the only intended caller.
            #[allow(dead_code)]
            pub(crate) fn from_committed_payload(payload: T) -> Self {
                Self { payload }
            }
        }
    };
}

crate_committed_constructor!(Accepted);
crate_committed_constructor!(Fact);
crate_committed_constructor!(Projected);

impl<T> sealed::PreparableOutcome for Accepted<T> {}
impl<T> sealed::PreparableOutcome for Fact<T> {}

pub(crate) trait FrameworkCommandOutcome: CommandOutcome {
    fn finalize(payload: Self::Payload) -> Self;
}

macro_rules! framework_outcome {
    ($wrapper:ident) => {
        impl<T> FrameworkCommandOutcome for $wrapper<T>
        where
            T: GraphqlOutputType + Serialize + Send + Sync + 'static,
        {
            fn finalize(payload: T) -> Self {
                Self::from_committed_payload(payload)
            }
        }
    };
}

framework_outcome!(Accepted);
framework_outcome!(Fact);
framework_outcome!(Projected);

/// Sealed type-level contract implemented by committed command outcomes.
pub trait CommandOutcome: sealed::Outcome + Send + Sync + 'static {
    type Payload: GraphqlOutputType + Serialize + Send + Sync + 'static;
    const CONSISTENCY: CommandConsistency;

    fn payload(&self) -> &Self::Payload;
}

/// Error produced while serializing a completion before commit I/O.
#[derive(Debug)]
pub enum PrepareCommandError {
    Serialize(serde_json::Error),
}

impl std::fmt::Display for PrepareCommandError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialize(error) => {
                write!(formatter, "command payload serialization failed: {error}")
            }
        }
    }
}

impl std::error::Error for PrepareCommandError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Serialize(error) => Some(error),
        }
    }
}

impl From<serde_json::Error> for PrepareCommandError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialize(error)
    }
}

/// A serialized command completion waiting for task 5's atomic committer.
///
/// Preparing is deliberately separate from returning a committed outcome: it
/// proves serialization before transaction I/O while keeping both the durable
/// outcome and the declaration-owned confirmation plan outside application
/// handler control.
pub struct PreparedCommand<K: CommandOutcome> {
    payload: K::Payload,
    serialized_payload: serde_json::Value,
    _outcome: PhantomData<fn() -> K>,
}

impl<K: CommandOutcome> PreparedCommand<K> {
    fn prepare_payload(payload: K::Payload) -> Result<Self, PrepareCommandError> {
        let serialized_payload = serde_json::to_value(&payload)?;
        Ok(Self {
            payload,
            serialized_payload,
            _outcome: PhantomData,
        })
    }

    pub fn consistency(&self) -> CommandConsistency {
        K::CONSISTENCY
    }

    pub fn serialized_payload(&self) -> &serde_json::Value {
        &self.serialized_payload
    }

    /// Task 5's durable committer is the sole intended consumer.
    #[allow(dead_code)]
    pub(crate) fn finalize_after_commit(self) -> (K, serde_json::Value)
    where
        K: FrameworkCommandOutcome,
    {
        (K::finalize(self.payload), self.serialized_payload)
    }
}

impl<K> PreparedCommand<K>
where
    K: CommandOutcome + sealed::PreparableOutcome,
{
    /// Prepare an accepted or fact payload for task 5's durable committer.
    /// Projected results require task 5's staged transactional proof and do
    /// not implement the private preparation capability.
    pub fn prepare(payload: K::Payload) -> Result<Self, PrepareCommandError> {
        Self::prepare_payload(payload)
    }
}

/// Generator evaluated exactly once into the canonical command input before
/// hashing, optimistic overlay evaluation, or dispatch (runtime task 9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputDefaultGenerator {
    UuidV7,
    Ulid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CommandInputDefault {
    pub path: Vec<String>,
    pub generator: InputDefaultGenerator,
}

/// Portable value expression in a command effect.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum EffectExpression {
    Input {
        path: Vec<String>,
    },
    TrustedPreset {
        name: String,
    },
    Constant {
        value: serde_json::Value,
    },
    /// SQL null, emitted only by the type-checked `null()` expression.
    Null,
    /// Construction-time serialization failure. This private sentinel is
    /// rejected before contract fingerprinting or Surface/manifest emission.
    #[serde(skip)]
    InvalidConstant {
        error: String,
    },
}

/// One model-field assignment in portable effect IR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EffectFieldValue {
    pub field: String,
    pub value: EffectExpression,
}

/// A complete, ordered model key in portable effect IR.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EffectKey {
    pub fields: Vec<EffectFieldValue>,
}

/// One declaration-owned projector/model/key confirmation target.
///
/// Task 5 resolves these expressions from the retained canonical GraphQL wire
/// input before commit I/O, then commits the finite resolved obligations
/// atomically with the command ledger/fact. Handlers cannot add, remove, or
/// rewrite targets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CommandProjectionConfirmation {
    pub projector: String,
    pub model: String,
    pub key: EffectKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partition: Option<EffectExpression>,
    /// Frozen declaration identity used for server-side topology validation
    /// and typed service binding. It is intentionally absent from role client
    /// manifests, whose projector catalog already carries authorized topology.
    #[serde(skip_serializing)]
    projector_topology: ProjectorTopologyIdentity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProjectorTopologyIdentity {
    name: String,
    facts: Vec<String>,
    models: Vec<String>,
}

impl ProjectorTopologyIdentity {
    fn new(name: &str, facts: &[String], models: &[String]) -> Self {
        let mut facts = facts.to_vec();
        facts.sort();
        facts.dedup();
        let mut models = models.to_vec();
        models.sort();
        models.dedup();
        Self {
            name: name.to_string(),
            facts,
            models,
        }
    }

    fn canonical_value(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "facts": self.facts,
            "models": self.models,
        })
    }
}

impl CommandProjectionConfirmation {
    pub(crate) fn canonical_value(&self) -> serde_json::Value {
        serde_json::json!({
            "projector": self.projector,
            "projector_topology": self.projector_topology.canonical_value(),
            "model": self.model,
            "key": self.key,
            "partition": self.partition,
        })
    }

    pub(crate) fn topology_matches(&self, name: &str, facts: &[String], models: &[String]) -> bool {
        self.projector_topology == ProjectorTopologyIdentity::new(name, facts, models)
    }
}

/// Relationship identity used by link and unlink effects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EffectRelationship {
    pub source_model: String,
    pub field: String,
    pub target_model: String,
}

/// Closed portable command-effect operation set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum CommandEffect {
    Upsert {
        model: String,
        key: EffectKey,
        fields: Vec<EffectFieldValue>,
    },
    Patch {
        model: String,
        key: EffectKey,
        fields: Vec<EffectFieldValue>,
    },
    Delete {
        model: String,
        key: EffectKey,
    },
    Link {
        relationship: EffectRelationship,
        source: EffectKey,
        target: EffectKey,
    },
    Unlink {
        relationship: EffectRelationship,
        source: EffectKey,
        target: EffectKey,
    },
    InvalidateModel {
        model: String,
    },
    InvalidateRelationship {
        relationship: EffectRelationship,
        source: EffectKey,
    },
}

/// What the client must do when the declared operations cannot prove safety.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CommandEffectFallback {
    /// No local invention: mark affected selections stale and revalidate.
    Revalidate,
}

/// Version-independent portable effect declaration attached to one command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CommandEffects {
    pub operations: Vec<CommandEffect>,
    pub fallback: CommandEffectFallback,
}

impl CommandEffects {
    pub(crate) fn new(operations: impl IntoIterator<Item = CommandEffect>) -> Self {
        Self {
            operations: operations.into_iter().collect(),
            fallback: CommandEffectFallback::Revalidate,
        }
    }

    pub(crate) fn revalidate() -> Self {
        Self::new([])
    }

    pub(crate) fn canonicalize(&mut self) {
        for operation in &mut self.operations {
            match operation {
                CommandEffect::Upsert { fields, .. } | CommandEffect::Patch { fields, .. } => {
                    fields.sort_by(|left, right| left.field.cmp(&right.field));
                }
                CommandEffect::Delete { .. }
                | CommandEffect::Link { .. }
                | CommandEffect::Unlink { .. }
                | CommandEffect::InvalidateModel { .. }
                | CommandEffect::InvalidateRelationship { .. } => {}
            }
        }
    }

    fn invalid_constant_error(&self) -> Option<&str> {
        self.operations
            .iter()
            .find_map(|operation| match operation {
                CommandEffect::Upsert { key, fields, .. }
                | CommandEffect::Patch { key, fields, .. } => invalid_key_constant(key)
                    .or_else(|| fields.iter().find_map(invalid_field_constant)),
                CommandEffect::Delete { key, .. } => invalid_key_constant(key),
                CommandEffect::Link { source, target, .. }
                | CommandEffect::Unlink { source, target, .. } => {
                    invalid_key_constant(source).or_else(|| invalid_key_constant(target))
                }
                CommandEffect::InvalidateRelationship { source, .. } => {
                    invalid_key_constant(source)
                }
                CommandEffect::InvalidateModel { .. } => None,
            })
    }
}

fn invalid_expression_constant(expression: &EffectExpression) -> Option<&str> {
    match expression {
        EffectExpression::InvalidConstant { error } => Some(error),
        EffectExpression::Input { .. }
        | EffectExpression::TrustedPreset { .. }
        | EffectExpression::Constant { .. }
        | EffectExpression::Null => None,
    }
}

fn invalid_field_constant(field: &EffectFieldValue) -> Option<&str> {
    invalid_expression_constant(&field.value)
}

fn invalid_key_constant(key: &EffectKey) -> Option<&str> {
    key.fields.iter().find_map(invalid_field_constant)
}

fn invalid_confirmation_constant(confirmation: &CommandProjectionConfirmation) -> Option<&str> {
    invalid_key_constant(&confirmation.key).or_else(|| {
        confirmation
            .partition
            .as_ref()
            .and_then(invalid_expression_constant)
    })
}

/// Wire-shape proofs emitted by derives and erased after compatibility checks.
#[doc(hidden)]
pub struct EffectWireChecked;
#[doc(hidden)]
pub struct EffectWireLiteral;
#[doc(hidden)]
pub struct EffectWireString;
#[doc(hidden)]
pub struct EffectWireBoolean;
#[doc(hidden)]
pub struct EffectWireBigInt;
#[doc(hidden)]
pub struct EffectWireFloat;
#[doc(hidden)]
pub struct EffectWireJson;
#[doc(hidden)]
pub struct EffectWireBytea;
#[doc(hidden)]
pub struct EffectWireTimestamp;
#[doc(hidden)]
pub struct EffectWireList;
#[doc(hidden)]
pub struct EffectWireObject;
#[doc(hidden)]
pub struct EffectWireUnsupported;

/// Closed compile-time compatibility relation between GraphQL input wire
/// shapes and read-model scalar codecs.
#[doc(hidden)]
pub trait EffectWireCompatible<Target> {}

macro_rules! exact_effect_wire_compatibility {
    ($($wire:ty),+ $(,)?) => {
        $(impl EffectWireCompatible<$wire> for $wire {})+
    };
}

exact_effect_wire_compatibility!(
    EffectWireString,
    EffectWireBoolean,
    EffectWireBigInt,
    EffectWireFloat,
    EffectWireJson,
    EffectWireBytea,
    EffectWireTimestamp,
    EffectWireList,
    EffectWireObject,
    EffectWireUnsupported,
);

// JSON model columns deliberately accept a complete input container as their
// leaf value. Other scalar codecs never accept list/object wire shapes.
impl EffectWireCompatible<EffectWireJson> for EffectWireList {}
impl EffectWireCompatible<EffectWireJson> for EffectWireObject {}

// Constants and explicit null retain exact Rust value typing; Surface
// validation checks their serialized scalar/null representation.
impl<Target> EffectWireCompatible<Target> for EffectWireLiteral {}

/// Typed portable expression used only while constructing erased effect IR.
#[doc(hidden)]
pub struct TypedEffectExpression<T, Wire = EffectWireChecked> {
    expression: EffectExpression,
    _value: PhantomData<fn() -> (T, Wire)>,
}

impl<T, Wire> TypedEffectExpression<T, Wire> {
    #[doc(hidden)]
    pub(crate) fn __into_ir(self) -> EffectExpression {
        self.expression
    }

    fn erase_wire(self) -> TypedEffectExpression<T> {
        TypedEffectExpression {
            expression: self.expression,
            _value: PhantomData,
        }
    }
}

/// Marker implemented only by `GraphqlInput` derive output in normal use.
///
/// The trait must be public because derive output lives in downstream crates.
/// All marker metadata is still revalidated against the final command Surface;
/// hand-written implementations cannot bypass runtime structural checks.
#[doc(hidden)]
pub struct EffectRequired;

#[doc(hidden)]
pub struct EffectNullable;

/// Derive-owned classification for a field that is a nested input object and
/// may therefore appear before another segment in an effect input path.
#[doc(hidden)]
pub struct EffectInputObjectKind;

/// Derive-owned classification for scalar and list fields. These fields are
/// valid leaves but cannot be traversed by effect input paths.
#[doc(hidden)]
pub struct EffectInputTerminalKind;

#[doc(hidden)]
pub trait EffectInputPathKind {}

impl EffectInputPathKind for EffectInputObjectKind {}
impl EffectInputPathKind for EffectInputTerminalKind {}

/// Implemented only for the derive-owned nested-object classification.
#[doc(hidden)]
pub trait EffectInputDescendableKind: EffectInputPathKind {}

impl EffectInputDescendableKind for EffectInputObjectKind {}

#[doc(hidden)]
pub trait EffectPathNullability {
    type Applied<T>;
}

impl EffectPathNullability for EffectRequired {
    type Applied<T> = T;
}

impl EffectPathNullability for EffectNullable {
    type Applied<T> = Option<T>;
}

#[doc(hidden)]
pub trait CombineEffectNullability<Other: EffectPathNullability> {
    type Output: EffectPathNullability;
}

impl CombineEffectNullability<EffectRequired> for EffectRequired {
    type Output = EffectRequired;
}

impl CombineEffectNullability<EffectNullable> for EffectRequired {
    type Output = EffectNullable;
}

impl CombineEffectNullability<EffectRequired> for EffectNullable {
    type Output = EffectNullable;
}

impl CombineEffectNullability<EffectNullable> for EffectNullable {
    type Output = EffectNullable;
}

#[doc(hidden)]
pub trait EffectInputFieldMarker {
    type Input: 'static;
    type Value;
    type NonNullValue;
    type Nullability: EffectPathNullability;
    type PathKind: EffectInputPathKind;
    type Wire;
    /// Unwrapped object type used when descending through a nested input.
    type Nested: 'static;
    fn path() -> Vec<&'static str>;
}

/// Type-level composition of two derive-generated input-field markers.
#[doc(hidden)]
pub struct EffectInputPath<Outer, Inner>(PhantomData<fn(Outer) -> Inner>);

impl<Outer, Inner> EffectInputFieldMarker for EffectInputPath<Outer, Inner>
where
    Outer: EffectInputFieldMarker,
    Inner: EffectInputFieldMarker<Input = Outer::Nested>,
    Outer::PathKind: EffectInputDescendableKind,
    Outer::Nullability: CombineEffectNullability<Inner::Nullability>,
{
    type Input = Outer::Input;
    type Value = <<Outer::Nullability as CombineEffectNullability<
        Inner::Nullability,
    >>::Output as EffectPathNullability>::Applied<Inner::NonNullValue>;
    type NonNullValue = Inner::NonNullValue;
    type Nullability = <Outer::Nullability as CombineEffectNullability<Inner::Nullability>>::Output;
    type PathKind = Inner::PathKind;
    type Wire = Inner::Wire;
    type Nested = Inner::Nested;

    fn path() -> Vec<&'static str> {
        let mut path = Outer::path();
        path.extend(Inner::path());
        path
    }
}

/// Convert a derive-generated input marker into a typed portable expression.
#[doc(hidden)]
pub fn __effect_input<I, F>() -> TypedEffectExpression<F::Value, F::Wire>
where
    I: 'static,
    F: EffectInputFieldMarker<Input = I>,
{
    TypedEffectExpression {
        expression: EffectExpression::Input {
            path: F::path().into_iter().map(str::to_string).collect(),
        },
        _value: PhantomData,
    }
}

/// Wraps a serializer so every nested value is visited through the same
/// portable-JSON checks. This is intentionally one pass: even a stateful custom
/// `Serialize` implementation cannot validate one value and emit another.
struct StrictPortableJsonSerializer<S>(S);

struct StrictPortableJsonValue<'a, T: ?Sized>(&'a T);

impl<T> Serialize for StrictPortableJsonValue<'_, T>
where
    T: ?Sized + Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(StrictPortableJsonSerializer(serializer))
    }
}

macro_rules! delegate_portable_scalar {
    ($($method:ident($value:ty)),+ $(,)?) => {
        $(
            fn $method(self, value: $value) -> Result<Self::Ok, Self::Error> {
                self.0.$method(value)
            }
        )+
    };
}

impl<S> serde::Serializer for StrictPortableJsonSerializer<S>
where
    S: serde::Serializer,
{
    type Ok = S::Ok;
    type Error = S::Error;
    type SerializeSeq = StrictSerializeSeq<S::SerializeSeq>;
    type SerializeTuple = StrictSerializeTuple<S::SerializeTuple>;
    type SerializeTupleStruct = StrictSerializeTupleStruct<S::SerializeTupleStruct>;
    type SerializeTupleVariant = StrictSerializeTupleVariant<S::SerializeTupleVariant>;
    type SerializeMap = StrictSerializeMap<S::SerializeMap>;
    type SerializeStruct = StrictSerializeStruct<S::SerializeStruct>;
    type SerializeStructVariant = StrictSerializeStructVariant<S::SerializeStructVariant>;

    delegate_portable_scalar! {
        serialize_bool(bool),
        serialize_i8(i8),
        serialize_i16(i16),
        serialize_i32(i32),
        serialize_i64(i64),
        serialize_i128(i128),
        serialize_u8(u8),
        serialize_u16(u16),
        serialize_u32(u32),
        serialize_u64(u64),
        serialize_u128(u128),
        serialize_char(char),
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        if !value.is_finite() {
            return Err(<S::Error as serde::ser::Error>::custom(
                "non-finite f32/f64 constants cannot be represented in portable JSON",
            ));
        }
        self.0.serialize_f32(value)
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        if !value.is_finite() {
            return Err(<S::Error as serde::ser::Error>::custom(
                "non-finite f32/f64 constants cannot be represented in portable JSON",
            ));
        }
        self.0.serialize_f64(value)
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_str(value)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_bytes(value)
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_none()
    }

    fn serialize_some<T>(self, value: &T) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_some(&StrictPortableJsonValue(value))
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit()
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_struct(name)
    }

    fn serialize_unit_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        self.0.serialize_unit_variant(name, variant_index, variant)
    }

    fn serialize_newtype_struct<T>(
        self,
        name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0
            .serialize_newtype_struct(name, &StrictPortableJsonValue(value))
    }

    fn serialize_newtype_variant<T>(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_newtype_variant(
            name,
            variant_index,
            variant,
            &StrictPortableJsonValue(value),
        )
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        self.0.serialize_seq(len).map(StrictSerializeSeq)
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.0.serialize_tuple(len).map(StrictSerializeTuple)
    }

    fn serialize_tuple_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.0
            .serialize_tuple_struct(name, len)
            .map(StrictSerializeTupleStruct)
    }

    fn serialize_tuple_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.0
            .serialize_tuple_variant(name, variant_index, variant, len)
            .map(StrictSerializeTupleVariant)
    }

    fn serialize_map(self, len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        self.0.serialize_map(len).map(StrictSerializeMap)
    }

    fn serialize_struct(
        self,
        name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.0
            .serialize_struct(name, len)
            .map(StrictSerializeStruct)
    }

    fn serialize_struct_variant(
        self,
        name: &'static str,
        variant_index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.0
            .serialize_struct_variant(name, variant_index, variant, len)
            .map(StrictSerializeStructVariant)
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

struct StrictSerializeSeq<S>(S);

impl<S> SerializeSeq for StrictSerializeSeq<S>
where
    S: SerializeSeq,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_element(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeTuple<S>(S);

impl<S> SerializeTuple for StrictSerializeTuple<S>
where
    S: SerializeTuple,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_element<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_element(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeTupleStruct<S>(S);

impl<S> SerializeTupleStruct for StrictSerializeTupleStruct<S>
where
    S: SerializeTupleStruct,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeTupleVariant<S>(S);

impl<S> SerializeTupleVariant for StrictSerializeTupleVariant<S>
where
    S: SerializeTupleVariant,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeMap<S>(S);

impl<S> SerializeMap for StrictSerializeMap<S>
where
    S: SerializeMap,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_key<T>(&mut self, key: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_key(&StrictPortableJsonValue(key))
    }

    fn serialize_value<T>(&mut self, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_value(&StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeStruct<S>(S);

impl<S> SerializeStruct for StrictSerializeStruct<S>
where
    S: SerializeStruct,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(key, &StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

struct StrictSerializeStructVariant<S>(S);

impl<S> SerializeStructVariant for StrictSerializeStructVariant<S>
where
    S: SerializeStructVariant,
{
    type Ok = S::Ok;
    type Error = S::Error;

    fn serialize_field<T>(&mut self, key: &'static str, value: &T) -> Result<(), Self::Error>
    where
        T: ?Sized + Serialize,
    {
        self.0.serialize_field(key, &StrictPortableJsonValue(value))
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        self.0.end()
    }
}

/// Serialize a deterministic value emitted by `command_effects!` while
/// retaining its Rust type for assignment checking. Serialization failures are
/// retained as private invalid IR and reported as configuration errors; a
/// declaration must never panic while it is being assembled.
#[doc(hidden)]
pub fn __effect_constant<T: Serialize>(value: T) -> TypedEffectExpression<T, EffectWireLiteral> {
    let expression = match StrictPortableJsonValue(&value).serialize(serde_json::value::Serializer)
    {
        Ok(value) => EffectExpression::Constant { value },
        Err(error) => EffectExpression::InvalidConstant {
            error: error.to_string(),
        },
    };
    TypedEffectExpression {
        expression,
        _value: PhantomData,
    }
}

/// Explicit nullable constant for optional model fields.
#[doc(hidden)]
pub fn __effect_null<T>() -> TypedEffectExpression<Option<T>, EffectWireLiteral> {
    TypedEffectExpression {
        expression: EffectExpression::Null,
        _value: PhantomData,
    }
}

/// Assignment compatibility implemented by framework expression types.
/// Non-null values may flow into nullable fields; nullable values never flow
/// into non-null fields. Key expressions remain exact and do not use this
/// conversion.
#[doc(hidden)]
pub trait EffectAssignmentExpression<Target> {
    type Wire;
    fn into_assignment(self) -> TypedEffectExpression<Target, Self::Wire>;
}

impl<T, Wire> EffectAssignmentExpression<T> for TypedEffectExpression<T, Wire> {
    type Wire = Wire;

    fn into_assignment(self) -> TypedEffectExpression<T, Wire> {
        self
    }
}

impl<T, Wire> EffectAssignmentExpression<Option<T>> for TypedEffectExpression<T, Wire> {
    type Wire = Wire;

    fn into_assignment(self) -> TypedEffectExpression<Option<T>, Wire> {
        TypedEffectExpression {
            expression: self.expression,
            _value: PhantomData,
        }
    }
}

/// Marker implemented by a `ReadModel` derive for one concrete model field.
#[doc(hidden)]
pub trait EffectModelFieldMarker {
    type Model: RelationalReadModel;
    type Value;
    type Wire;
    const FIELD: &'static str;
}

/// Marker implemented by a `ReadModel` derive for one relationship.
#[doc(hidden)]
pub trait EffectRelationshipMarker {
    type Source: RelationalReadModel;
    type Target: RelationalReadModel;
    const FIELD: &'static str;
}

/// Typed model key generated as a named struct by `#[derive(ReadModel)]`.
#[doc(hidden)]
pub struct TypedEffectKey<M> {
    key: EffectKey,
    _model: PhantomData<fn() -> M>,
}

/// Opaque, typed confirmation target created from a projector declaration.
///
/// The projector object and model key are reused directly, avoiding a second
/// application-maintained projector/model string join. Final Surface building
/// still validates the target against authorized projector topology.
#[doc(hidden)]
pub struct CompiledProjectionConfirmation<I>(CommandProjectionConfirmation, PhantomData<fn(I)>);

impl<I> CompiledProjectionConfirmation<I> {
    /// Partition the expected projector progress by a deterministic string/ID
    /// expression from the same command input.
    pub fn partition<Wire>(mut self, partition: TypedEffectExpression<String, Wire>) -> Self
    where
        Wire: EffectWireCompatible<EffectWireString>,
    {
        self.0.partition = Some(partition.__into_ir());
        self
    }
}

pub(crate) fn projection_confirmation<M: RelationalReadModel>(
    projector: &str,
    facts: &[String],
    models: &[String],
    key: TypedEffectKey<M>,
) -> CommandProjectionConfirmation {
    CommandProjectionConfirmation {
        projector: projector.to_string(),
        model: M::schema().model_name.clone(),
        key: key.key,
        partition: None,
        projector_topology: ProjectorTopologyIdentity::new(projector, facts, models),
    }
}

pub(crate) fn compiled_projection_confirmation<I, M: RelationalReadModel>(
    projector: &str,
    facts: &[String],
    models: &[String],
    key: TypedEffectKey<M>,
) -> CompiledProjectionConfirmation<I> {
    CompiledProjectionConfirmation(
        projection_confirmation(projector, facts, models, key),
        PhantomData,
    )
}

/// Compile-checked, declaration-owned projection confirmation plan.
///
/// The input type parameter prevents a plan built for a lookalike input type
/// from being attached to a different command declaration.
pub struct CompiledConfirmationPlan<I>(Vec<CommandProjectionConfirmation>, PhantomData<fn(I)>);

#[doc(hidden)]
pub fn __command_confirmations<I>(
    confirmations: impl IntoIterator<Item = CompiledProjectionConfirmation<I>>,
) -> CompiledConfirmationPlan<I> {
    CompiledConfirmationPlan(
        confirmations
            .into_iter()
            .map(|confirmation| confirmation.0)
            .collect(),
        PhantomData,
    )
}

/// Opaque key field emitted by a `ReadModel` derive. Application code cannot
/// assemble raw effect IR through the public typed-command API.
#[doc(hidden)]
pub struct CompiledEffectKeyField<M>(EffectFieldValue, PhantomData<fn() -> M>);

#[doc(hidden)]
pub fn __effect_key_field<F>(
    value: TypedEffectExpression<F::Value>,
) -> CompiledEffectKeyField<F::Model>
where
    F: EffectModelFieldMarker,
{
    CompiledEffectKeyField(
        EffectFieldValue {
            field: F::FIELD.to_string(),
            value: value.__into_ir(),
        },
        PhantomData,
    )
}

/// Prove one generated key expression's wire compatibility, then erase the
/// proof so derive-generated composite key structs need no wire generics.
#[doc(hidden)]
pub fn __effect_key_assignment<F, Wire>(
    value: TypedEffectExpression<F::Value, Wire>,
) -> TypedEffectExpression<F::Value>
where
    F: EffectModelFieldMarker,
    Wire: EffectWireCompatible<F::Wire>,
{
    value.erase_wire()
}

/// Assemble a typed model key from derive-generated field markers. The final
/// Surface validator additionally requires the exact ordered primary key.
#[doc(hidden)]
pub fn __effect_key<M: RelationalReadModel>(
    fields: Vec<CompiledEffectKeyField<M>>,
) -> TypedEffectKey<M> {
    TypedEffectKey {
        key: EffectKey {
            fields: fields.into_iter().map(|field| field.0).collect(),
        },
        _model: PhantomData,
    }
}

/// Typed relationship marker generated by `#[derive(ReadModel)]`.
#[doc(hidden)]
pub struct TypedEffectRelationship<S, T> {
    relationship: EffectRelationship,
    _models: PhantomData<fn(S) -> T>,
}

impl<S, T> TypedEffectRelationship<S, T> {}

/// Convert a derive-generated relationship marker into opaque typed IR.
#[doc(hidden)]
pub fn __effect_relationship<R>() -> TypedEffectRelationship<R::Source, R::Target>
where
    R: EffectRelationshipMarker,
{
    TypedEffectRelationship {
        relationship: EffectRelationship {
            source_model: R::Source::schema().model_name.clone(),
            field: R::FIELD.to_string(),
            target_model: R::Target::schema().model_name.clone(),
        },
        _models: PhantomData,
    }
}

/// Type-checked field assignment helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_assignment<F, E>(value: E) -> CompiledEffectFieldValue<F::Model>
where
    F: EffectModelFieldMarker,
    E: EffectAssignmentExpression<F::Value>,
    E::Wire: EffectWireCompatible<F::Wire>,
{
    let value = value.into_assignment();
    CompiledEffectFieldValue(
        EffectFieldValue {
            field: F::FIELD.to_string(),
            value: value.__into_ir(),
        },
        PhantomData,
    )
}

/// Opaque type-checked field assignment emitted by `command_effects!`.
#[doc(hidden)]
pub struct CompiledEffectFieldValue<M>(EffectFieldValue, PhantomData<fn() -> M>);

/// Opaque compiled effect operation. Only generated typed helpers can produce
/// one; raw IR is not accepted by [`TypedCommand::effects`].
#[doc(hidden)]
pub struct CompiledEffectOperation(CommandEffect);

/// One compile-checked generated canonical-input default.
#[doc(hidden)]
pub struct CompiledInputDefault<I>(CommandInputDefault, PhantomData<fn(I)>);

/// Declaration-owned generated defaults for one exact command input type.
pub struct CompiledInputDefaults<I>(Vec<CommandInputDefault>, PhantomData<fn(I)>);

#[doc(hidden)]
pub fn __input_default_uuid_v7<I, F>() -> CompiledInputDefault<I>
where
    I: 'static,
    F: EffectInputFieldMarker<Input = I, Value = String>,
{
    CompiledInputDefault(
        CommandInputDefault {
            path: F::path().into_iter().map(str::to_string).collect(),
            generator: InputDefaultGenerator::UuidV7,
        },
        PhantomData,
    )
}

#[doc(hidden)]
pub fn __input_default_ulid<I, F>() -> CompiledInputDefault<I>
where
    I: 'static,
    F: EffectInputFieldMarker<Input = I, Value = String>,
{
    CompiledInputDefault(
        CommandInputDefault {
            path: F::path().into_iter().map(str::to_string).collect(),
            generator: InputDefaultGenerator::Ulid,
        },
        PhantomData,
    )
}

#[doc(hidden)]
pub fn __command_input_defaults<I>(
    defaults: impl IntoIterator<Item = CompiledInputDefault<I>>,
) -> CompiledInputDefaults<I> {
    CompiledInputDefaults(
        defaults.into_iter().map(|default| default.0).collect(),
        PhantomData,
    )
}

/// Opaque, compile-checked effect declaration returned by `command_effects!`.
pub struct CompiledCommandEffects<I>(CommandEffects, PhantomData<fn(I)>);

#[doc(hidden)]
pub fn __command_effects<I>(
    operations: impl IntoIterator<Item = CompiledEffectOperation>,
) -> CompiledCommandEffects<I> {
    CompiledCommandEffects(
        CommandEffects::new(operations.into_iter().map(|operation| operation.0)),
        PhantomData,
    )
}

/// Type-checked upsert helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_upsert<M: RelationalReadModel>(
    key: TypedEffectKey<M>,
    fields: Vec<CompiledEffectFieldValue<M>>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Upsert {
        model: M::schema().model_name.clone(),
        key: key.key,
        fields: fields.into_iter().map(|field| field.0).collect(),
    })
}

/// Type-checked patch helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_patch<M: RelationalReadModel>(
    key: TypedEffectKey<M>,
    fields: Vec<CompiledEffectFieldValue<M>>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Patch {
        model: M::schema().model_name.clone(),
        key: key.key,
        fields: fields.into_iter().map(|field| field.0).collect(),
    })
}

/// Type-checked delete helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_delete<M: RelationalReadModel>(key: TypedEffectKey<M>) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Delete {
        model: M::schema().model_name.clone(),
        key: key.key,
    })
}

/// Type-checked link helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_link<S: RelationalReadModel, T: RelationalReadModel>(
    relationship: TypedEffectRelationship<S, T>,
    source: TypedEffectKey<S>,
    target: TypedEffectKey<T>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Link {
        relationship: relationship.relationship,
        source: source.key,
        target: target.key,
    })
}

/// Type-checked unlink helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_unlink<S: RelationalReadModel, T: RelationalReadModel>(
    relationship: TypedEffectRelationship<S, T>,
    source: TypedEffectKey<S>,
    target: TypedEffectKey<T>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::Unlink {
        relationship: relationship.relationship,
        source: source.key,
        target: target.key,
    })
}

/// Type-checked model invalidation helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_invalidate_model<M: RelationalReadModel>() -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::InvalidateModel {
        model: M::schema().model_name.clone(),
    })
}

/// Type-checked relationship invalidation helper used by the effect proc macro.
#[doc(hidden)]
pub fn __effect_invalidate_relationship<S: RelationalReadModel, T: RelationalReadModel>(
    relationship: TypedEffectRelationship<S, T>,
    source: TypedEffectKey<S>,
) -> CompiledEffectOperation {
    CompiledEffectOperation(CommandEffect::InvalidateRelationship {
        relationship: relationship.relationship,
        source: source.key,
    })
}

/// Stable erased metadata shared by the executable service and GraphQL engine.
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
}

impl TypedCommandContract {
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
        serde_json::json!({
            "name": self.name,
            "field_name": self.field_name,
            "roles": roles,
            "input": canonical_graphql_type(&self.input),
            "output": canonical_graphql_type(&self.output),
            "consistency": self.consistency,
            "input_defaults": input_defaults,
            "effects": effects,
            "confirmations": confirmations,
        })
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
                CommandConsistency::Fact if contract.confirmations.is_empty() => {
                    return Err(format!(
                        "typed fact command `{}` must declare at least one expected projector confirmation",
                        contract.name
                    ));
                }
                CommandConsistency::Projected if !contract.confirmations.is_empty() => {
                    return Err(format!(
                        "typed projected command `{}` cannot declare asynchronous projector confirmations",
                        contract.name
                    ));
                }
                CommandConsistency::Fact
                | CommandConsistency::Accepted
                | CommandConsistency::Projected => {}
            }
            if let Some(error) = contract.effects.invalid_constant_error().or_else(|| {
                contract
                    .confirmations
                    .iter()
                    .find_map(invalid_confirmation_constant)
            }) {
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

        let bytes = serde_json::to_vec(&serde_json::json!({
            "service_id": service_id,
            "commands": canonical,
        }))
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
    let output = K::Payload::graphql_type();
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

    /// Declare the finite projector/model/key progress that confirms this fact.
    /// `Fact<_>` commands require at least one confirmation. `Accepted<_>` may
    /// omit the plan (terminal accepted) or provide one (pending projection).
    /// `Projected<_>` commands cannot carry asynchronous confirmations.
    pub fn confirmations(mut self, confirmations: CompiledConfirmationPlan<I>) -> Self {
        self.contract.confirmations = confirmations.0;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::{GraphqlTypeDef, GraphqlTypeField};
    use serde::Deserialize;

    #[allow(dead_code)]
    #[derive(Deserialize)]
    struct Input {
        id: String,
    }

    impl GraphqlInputType for Input {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "Input",
                vec![GraphqlTypeField {
                    name: "id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
            .with_type_id(TypeId::of::<Self>())
        }
    }

    #[derive(Serialize)]
    struct Payload {
        id: String,
    }

    impl GraphqlOutputType for Payload {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "Payload",
                vec![GraphqlTypeField {
                    name: "id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
            .with_type_id(TypeId::of::<Self>())
        }
    }

    #[test]
    fn preparation_serializes_and_retains_the_typed_payload_until_commit() {
        let prepared = PreparedCommand::<Fact<Payload>>::prepare(Payload {
            id: "todo-1".into(),
        })
        .unwrap();
        assert_eq!(prepared.consistency(), CommandConsistency::Fact);
        assert_eq!(prepared.serialized_payload()["id"], "todo-1");
        let (committed, serialized) = prepared.finalize_after_commit();
        assert_eq!(committed.payload().id, "todo-1");
        assert_eq!(serialized["id"], "todo-1");
    }

    #[test]
    fn binding_rejects_missing_graphql_type_ids() {
        let mut contract = typed_command::<Input, Accepted<Payload>>("todo.create").into_contract();
        contract.input.type_id = None;
        let error = TypedServiceCommandBinding::from_contracts("todos", &[contract]).unwrap_err();
        assert!(error.contains("input GraphQL metadata is missing"));
    }

    #[test]
    fn binding_canonicalizes_fields_and_roles_but_preserves_effect_order() {
        let mut first = typed_command::<Input, Accepted<Payload>>("todo.create")
            .roles(["writer", "admin"])
            .into_contract();
        first.input.fields.push(GraphqlTypeField {
            name: "z_extra".into(),
            type_name: "String".into(),
            nullable: true,
            list: false,
            item_nullable: false,
            nested: None,
        });
        first.effects.operations = vec![
            CommandEffect::InvalidateModel {
                model: "Zed".into(),
            },
            CommandEffect::InvalidateModel {
                model: "Alpha".into(),
            },
        ];

        let mut second = first.clone();
        second.roles.reverse();
        second.input.fields.reverse();
        let first = TypedServiceCommandBinding::from_contracts("todos", &[first]).unwrap();
        let second = TypedServiceCommandBinding::from_contracts("todos", &[second]).unwrap();
        assert_eq!(first, second);

        let mut reordered = typed_command::<Input, Accepted<Payload>>("todo.create")
            .roles(["writer", "admin"])
            .into_contract();
        reordered.input.fields.push(GraphqlTypeField {
            name: "z_extra".into(),
            type_name: "String".into(),
            nullable: true,
            list: false,
            item_nullable: false,
            nested: None,
        });
        reordered.effects.operations = vec![
            CommandEffect::InvalidateModel {
                model: "Alpha".into(),
            },
            CommandEffect::InvalidateModel {
                model: "Zed".into(),
            },
        ];
        let reordered = TypedServiceCommandBinding::from_contracts("todos", &[reordered]).unwrap();
        assert_ne!(
            first.structural_fingerprint,
            reordered.structural_fingerprint
        );
    }
}
