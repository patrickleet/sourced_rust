use std::marker::PhantomData;

use serde::ser::{
    SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use serde::Serialize;

use super::effects::{
    CommandEffect, CommandEffects, EffectExpression, EffectFieldValue, EffectKey,
    EffectRelationship,
};
use super::projection_obligations::ProjectorTopologyIdentity;
use super::projection_obligations::{
    CommandInputDefault, CommandProjectionConfirmation, InputDefaultGenerator,
};
use crate::projection_protocol::ProjectionPartitionSpec;
use crate::read_model::RelationalReadModel;

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

/// Declare a typed value supplied only by the server's verified Session.
///
/// The generated artifact retains this bounded descriptor name. It never
/// embeds a value supplied by the caller.
#[doc(hidden)]
pub fn __effect_trusted<T>(name: &'static str) -> TypedEffectExpression<T, EffectWireLiteral> {
    TypedEffectExpression {
        expression: EffectExpression::TrustedPreset {
            name: name.to_string(),
        },
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
pub struct CompiledProjectionConfirmation<I>(
    pub(super) CommandProjectionConfirmation,
    PhantomData<fn(I)>,
);

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
    partition: &ProjectionPartitionSpec,
    key: TypedEffectKey<M>,
) -> CommandProjectionConfirmation {
    CommandProjectionConfirmation {
        projector: projector.to_string(),
        model: M::schema().model_name.clone(),
        key: key.key,
        partition: match partition {
            ProjectionPartitionSpec::Constant { value } => Some(EffectExpression::Constant {
                value: value.clone(),
            }),
            ProjectionPartitionSpec::Unit | ProjectionPartitionSpec::InputPath { .. } => None,
        },
        projector_topology: ProjectorTopologyIdentity::new(projector, facts, models, partition),
        protocol_topology: None,
        schema: Some(M::schema()),
    }
}

pub(crate) fn compiled_projection_confirmation<I, M: RelationalReadModel>(
    projector: &str,
    facts: &[String],
    models: &[String],
    partition: &ProjectionPartitionSpec,
    key: TypedEffectKey<M>,
) -> CompiledProjectionConfirmation<I> {
    CompiledProjectionConfirmation(
        projection_confirmation(projector, facts, models, partition, key),
        PhantomData,
    )
}

/// Compile-checked, declaration-owned projection confirmation plan.
///
/// The input type parameter prevents a plan built for a lookalike input type
/// from being attached to a different command declaration.
pub struct CompiledConfirmationPlan<I>(
    pub(super) Vec<CommandProjectionConfirmation>,
    PhantomData<fn(I)>,
);

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
pub struct CompiledInputDefaults<I>(pub(super) Vec<CommandInputDefault>, PhantomData<fn(I)>);

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
pub struct CompiledCommandEffects<I>(pub(super) CommandEffects, PhantomData<fn(I)>);

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
