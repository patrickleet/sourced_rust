use std::any::Any;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::error::{ApplicationError, ApplicationResult};
use super::identity::{canonical_json, sha256_fingerprint, LogicalId};
use crate::graphql::command_contract::{CommandConsistency, CommandOutcome, TypedCommand};
use crate::graphql::{GraphqlInputType, GraphqlTypeDef};

/// Serializable GraphQL type field used by a portable command contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandTypeField {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub list: bool,
    pub item_nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nested: Option<Box<CommandTypeSpec>>,
}

/// Serializable GraphQL input/output type used by a portable command contract.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandTypeSpec {
    pub name: String,
    pub fields: Vec<CommandTypeField>,
}

pub type TypeSpec = CommandTypeSpec;

impl From<&GraphqlTypeDef> for CommandTypeSpec {
    fn from(definition: &GraphqlTypeDef) -> Self {
        Self {
            name: definition.name.clone(),
            fields: definition
                .fields
                .iter()
                .map(|field| CommandTypeField {
                    name: field.name.clone(),
                    type_name: field.type_name.clone(),
                    nullable: field.nullable,
                    list: field.list,
                    item_nullable: field.item_nullable,
                    nested: field.nested.as_deref().map(Self::from).map(Box::new),
                })
                .collect(),
        }
    }
}

/// One exact outward event identity referenced by a command declaration.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EventSpec {
    pub name: String,
    pub version: u64,
    pub body_type: String,
    pub body_version: u64,
    pub body_schema: String,
    pub body_fingerprint: String,
    pub body_codec: String,
    pub body_codec_version: u16,
}

/// The portable half of one typed command declaration.
///
/// The effect, default, confirmation, and projection values are copied from
/// the existing typed-command IR. They remain explicit JSON-shaped data; no
/// handler symbol, Rust `TypeId`, closure, or machine path is retained.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandSpec {
    pub id: String,
    pub field_name: String,
    pub roles: Vec<String>,
    pub input: CommandTypeSpec,
    pub output: CommandTypeSpec,
    pub consistency: CommandConsistency,
    pub defaults: serde_json::Value,
    pub effects: serde_json::Value,
    pub emits: Vec<EventSpec>,
    pub applies: serde_json::Value,
    pub projection_contract: serde_json::Value,
    /// Declaration-owned projector/model/key confirmations. These are the
    /// portable proof inputs; topology pointers and schemas are intentionally
    /// reduced to their canonical identities by `canonical_value()`.
    #[serde(default)]
    pub confirmations: Vec<serde_json::Value>,
    /// Canonical direct-projection proof material, if the outcome is Atomic.
    /// The erased Rust type identity is never copied into this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_projection: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_model: Option<String>,
    pub fingerprint: String,
}

impl CommandSpec {
    /// Construct a command spec from already-portable pieces.
    pub fn try_new(
        id: impl Into<String>,
        field_name: impl Into<String>,
        input: CommandTypeSpec,
        output: CommandTypeSpec,
        consistency: CommandConsistency,
    ) -> ApplicationResult<Self> {
        let id = LogicalId::try_new("command", id)?.into_string();
        let field_name = field_name.into();
        if field_name.is_empty() {
            return Err(ApplicationError::InvalidSpec(
                "command field name must not be empty".into(),
            ));
        }
        let mut spec = Self {
            id,
            field_name,
            roles: Vec::new(),
            input,
            output,
            consistency,
            defaults: serde_json::Value::Array(Vec::new()),
            effects: serde_json::Value::Null,
            emits: Vec::new(),
            applies: serde_json::Value::Array(Vec::new()),
            projection_contract: serde_json::Value::Null,
            confirmations: Vec::new(),
            direct_projection: None,
            projected_model: None,
            fingerprint: String::new(),
        };
        spec.refresh_fingerprint()?;
        spec.validate()?;
        Ok(spec)
    }

    /// Build a portable spec from the framework's existing typed declaration.
    pub fn from_typed_command<I, K>(command: &TypedCommand<I, K>) -> ApplicationResult<Self>
    where
        I: GraphqlInputType + serde::de::DeserializeOwned + Send + 'static,
        K: CommandOutcome,
    {
        let (_, contract) = command.clone().into_parts();
        Self::from_contract(&contract)
    }

    pub(crate) fn from_contract(
        contract: &crate::graphql::command_contract::TypedCommandContract,
    ) -> ApplicationResult<Self> {
        let projection_contract = serde_json::to_value(&contract.projections)?;
        let applies = serde_json::to_value(&contract.projections.previews)?;
        let confirmations = contract
            .confirmations
            .iter()
            .map(crate::graphql::command_contract::CommandProjectionConfirmation::canonical_value)
            .collect();
        let direct_projection = contract
            .direct_projection
            .as_ref()
            .map(crate::graphql::command_contract::CommandDirectProjectionTarget::canonical_value);
        let emits = contract
            .projections
            .selectors
            .iter()
            .map(|selector| EventSpec {
                name: selector.event_name().to_owned(),
                version: selector.event_version(),
                body_type: selector.body_type_name().to_owned(),
                body_version: selector.body_version(),
                body_schema: selector.body_schema().to_owned(),
                body_fingerprint: selector.body_fingerprint().to_owned(),
                body_codec: selector.body_codec().to_owned(),
                body_codec_version: selector.body_codec_version(),
            })
            .collect();
        let mut roles = contract.roles.clone();
        roles.sort();
        roles.dedup();
        let mut spec = Self {
            id: LogicalId::try_new("command", contract.name.clone())?.into_string(),
            field_name: contract.field_name.clone(),
            roles,
            input: CommandTypeSpec::from(&contract.input),
            output: CommandTypeSpec::from(&contract.output),
            consistency: contract.consistency,
            defaults: serde_json::to_value(&contract.input_defaults)?,
            effects: serde_json::to_value(&contract.effects)?,
            emits,
            applies,
            projection_contract,
            confirmations,
            direct_projection,
            projected_model: contract
                .projected_model
                .as_ref()
                .map(|projected| projected.model.clone()),
            fingerprint: String::new(),
        };
        spec.refresh_fingerprint()?;
        spec.validate()?;
        Ok(spec)
    }

    pub fn canonical_bytes(&self) -> ApplicationResult<Vec<u8>> {
        let mut value = serde_json::to_value(self)?;
        if let serde_json::Value::Object(fields) = &mut value {
            fields.insert(
                "fingerprint".into(),
                serde_json::Value::String(String::new()),
            );
        }
        serde_json::to_vec(&canonical_json(&value)).map_err(Into::into)
    }

    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    pub fn identity(&self) -> &str {
        &self.id
    }

    /// Validate the portable identity and schema surface without touching a
    /// handler or any runtime state.
    pub fn validate(&self) -> ApplicationResult<()> {
        LogicalId::try_new("command", self.id.clone())?;
        validate_text("command field", &self.field_name)?;
        validate_type("command input", &self.input)?;
        validate_type("command output", &self.output)?;
        for role in &self.roles {
            validate_text("command role", role)?;
        }
        for event in &self.emits {
            LogicalId::try_new("event", event.name.clone())?;
            validate_text("event body type", &event.body_type)?;
            validate_text("event body schema", &event.body_schema)?;
            validate_text("event body fingerprint", &event.body_fingerprint)?;
            validate_text("event body codec", &event.body_codec)?;
        }
        if let Some(model) = &self.projected_model {
            LogicalId::try_new("model", model.clone())?;
        }
        Ok(())
    }

    pub(crate) fn refresh_fingerprint(&mut self) -> ApplicationResult<()> {
        let bytes = self.canonical_bytes()?;
        self.fingerprint = sha256_fingerprint(&bytes);
        Ok(())
    }
}

fn validate_type(kind: &'static str, definition: &CommandTypeSpec) -> ApplicationResult<()> {
    validate_text(kind, &definition.name)?;
    for field in &definition.fields {
        validate_text("command field", &field.name)?;
        validate_text("command field type", &field.type_name)?;
        if let Some(nested) = &field.nested {
            validate_type(kind, nested)?;
        }
    }
    Ok(())
}

fn validate_text(kind: &'static str, value: &str) -> ApplicationResult<()> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.contains('\0')
        || value.contains("${")
        || value.starts_with('/')
        || value.contains('\\')
    {
        return Err(ApplicationError::InvalidIdentity {
            kind,
            value: value.into(),
            reason: "must be a non-empty portable value",
        });
    }
    Ok(())
}

/// Executable command material retained only at the heterogeneous runtime
/// boundary. Its handler is intentionally absent from serialization.
pub struct CommandMount {
    spec: CommandSpec,
    handler: Option<Arc<dyn Any + Send + Sync>>,
}

impl Clone for CommandMount {
    fn clone(&self) -> Self {
        Self {
            spec: self.spec.clone(),
            handler: self.handler.clone(),
        }
    }
}

impl std::fmt::Debug for CommandMount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CommandMount")
            .field("command", &self.spec.id)
            .field("executable", &self.is_executable())
            .finish()
    }
}

impl CommandMount {
    /// Create a contract-only mount with no executable handler.
    pub fn contract(spec: CommandSpec) -> Self {
        Self {
            spec,
            handler: None,
        }
    }

    /// Erase one executable handler without changing the portable spec.
    pub fn from_handler<H>(spec: CommandSpec, handler: H) -> Self
    where
        H: Send + Sync + 'static,
    {
        Self {
            spec,
            handler: Some(Arc::new(handler)),
        }
    }

    pub fn spec(&self) -> &CommandSpec {
        &self.spec
    }

    pub fn is_executable(&self) -> bool {
        self.handler.is_some()
    }

    /// Recover a handler only when the runtime owner already knows its type.
    pub fn downcast_handler<H: 'static>(&self) -> Option<&H> {
        self.handler.as_deref()?.downcast_ref::<H>()
    }
}

impl<I, K> TypedCommand<I, K>
where
    I: GraphqlInputType + serde::de::DeserializeOwned + Send + 'static,
    K: CommandOutcome,
{
    /// Compile the exact declaration into its portable, serializable spec.
    pub fn spec(&self) -> ApplicationResult<CommandSpec> {
        CommandSpec::from_typed_command(self)
    }
}
