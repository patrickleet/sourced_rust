//! Command mutations (Hasura-actions parity): typed GraphQL mutation fields
//! that dispatch through `CommandRequest` and never touch read-model tables.
//!
//! # Command client catalog
//!
//! [`GraphqlCommands::catalog`] exports a machine-readable registry used by
//! TypeScript generators so app code can call `todosCreate(input)` without
//! hand-authoring GraphQL mutation documents. GraphQL remains the wire.
//!
//! Optional [`ExposedCommand::client_reconcile`] hints tell generators how the
//! browser command pipeline should treat success payloads (`ack` / `fact` /
//! `projection`) and how to reconcile lists (subscription / none / …).

use serde::Serialize;

use super::command_contract::{
    CommandConsistency, CommandDirectProjectionTarget, CommandEffects, CommandInputDefault,
    CommandProjectedModel, CommandProjectionConfirmation, TypedCommandContract,
};
use super::surface::{
    SurfaceCommand, SurfaceCommandShape, SurfaceProjector, SurfaceTypeDef, SurfaceTypeField,
};
use super::types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};
use crate::projection_protocol::compile_projection_topology;

/// How the browser interprets a successful mutation payload.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientResultKind {
    /// Command accepted; payload is not a list row.
    Ack,
    /// Domain fact fields (id, status, …) — not a full projected row.
    Fact,
    /// Payload matches the GraphQL/RM row; apply to cache immediately.
    Projection,
    /// Ignore payload for cache purposes.
    None,
}

/// How the browser refreshes list/query cache after a successful command.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClientReconcileMode {
    None,
    Subscription,
    Refetch,
    Invalidate,
}

/// Client pipeline policy for one command (exported in the command catalog).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ClientReconcile {
    pub result: ClientResultSpec,
    pub reconcile: ClientReconcileSpec,
}

/// Nested `result.kind` shape matching the TS `CommandPolicy` type.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ClientResultSpec {
    pub kind: ClientResultKind,
}

/// Nested `reconcile.kind` shape matching the TS `CommandPolicy` type.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ClientReconcileSpec {
    pub kind: ClientReconcileMode,
}

impl ClientReconcile {
    /// Async projector topology: fact payload, no immediate refetch.
    pub fn fact() -> Self {
        Self {
            result: ClientResultSpec {
                kind: ClientResultKind::Fact,
            },
            reconcile: ClientReconcileSpec {
                kind: ClientReconcileMode::None,
            },
        }
    }

    /// Async projector + live subscription owns the list.
    pub fn fact_subscription() -> Self {
        Self {
            result: ClientResultSpec {
                kind: ClientResultKind::Fact,
            },
            reconcile: ClientReconcileSpec {
                kind: ClientReconcileMode::Subscription,
            },
        }
    }

    /// Same-tx RM write: mutation payload is the projected row.
    pub fn projection() -> Self {
        Self {
            result: ClientResultSpec {
                kind: ClientResultKind::Projection,
            },
            reconcile: ClientReconcileSpec {
                kind: ClientReconcileMode::None,
            },
        }
    }

    /// Ack-only success (no useful fact fields).
    pub fn ack() -> Self {
        Self {
            result: ClientResultSpec {
                kind: ClientResultKind::Ack,
            },
            reconcile: ClientReconcileSpec {
                kind: ClientReconcileMode::None,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExposedCommand {
    pub(crate) command_name: String,
    pub(crate) field_name: Option<String>,
    pub(crate) input: CommandInput,
    pub(crate) output: CommandOutput,
    pub(crate) roles: Vec<String>,
    pub(crate) client_reconcile: Option<ClientReconcile>,
    pub(crate) consistency: Option<CommandConsistency>,
    pub(crate) input_defaults: Vec<CommandInputDefault>,
    pub(crate) effects: Option<CommandEffects>,
    pub(crate) confirmations: Vec<CommandProjectionConfirmation>,
    pub(crate) projected_model: Option<CommandProjectedModel>,
    pub(crate) direct_projection: Option<CommandDirectProjectionTarget>,
    pub(crate) confirmation_unavailable: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum CommandInput {
    None,
    Json,
    Typed(GraphqlTypeDef),
}

#[derive(Clone, Debug)]
pub(crate) enum CommandOutput {
    Json,
    Typed(GraphqlTypeDef),
}

pub fn exposed_command() -> ExposedCommand {
    ExposedCommand {
        command_name: String::new(),
        field_name: None,
        input: CommandInput::None,
        output: CommandOutput::Json,
        roles: Vec::new(),
        client_reconcile: None,
        consistency: None,
        input_defaults: Vec::new(),
        effects: None,
        confirmations: Vec::new(),
        projected_model: None,
        direct_projection: None,
        confirmation_unavailable: false,
    }
}

impl ExposedCommand {
    pub fn field_name(mut self, name: &str) -> Self {
        self.field_name = Some(name.to_string());
        self
    }

    pub fn input<T: GraphqlInputType>(mut self) -> Self {
        self.input = CommandInput::Typed(T::graphql_type());
        self
    }

    pub fn input_json(mut self) -> Self {
        self.input = CommandInput::Json;
        self
    }

    pub fn output<T: GraphqlOutputType>(mut self) -> Self {
        self.output = CommandOutput::Typed(T::graphql_type());
        self
    }

    pub fn roles<I: IntoIterator<Item = impl Into<String>>>(mut self, i: I) -> Self {
        self.roles = i.into_iter().map(Into::into).collect();
        self
    }

    /// Hint for generated TS command policies (result kind + reconcile mode).
    ///
    /// Omitted from the catalog when unset (backward compatible). Call-site
    /// options on the client still win over generated defaults.
    pub fn client_reconcile(mut self, policy: ClientReconcile) -> Self {
        self.client_reconcile = Some(policy);
        self
    }

    pub(crate) fn resolved_field_name(&self, command_name: &str) -> String {
        self.field_name.clone().unwrap_or_else(|| {
            command_name
                .chars()
                .map(|c| if c == '.' || c == '-' { '_' } else { c })
                .collect()
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct GraphqlCommands {
    pub(crate) commands: Vec<(String, ExposedCommand)>,
}

/// One field on a command input/output type (catalog / codegen).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CommandFieldCatalog {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub list: bool,
    pub item_nullable: bool,
}

/// Input or output object shape for a registered command.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CommandTypeCatalog {
    pub name: String,
    pub fields: Vec<CommandFieldCatalog>,
}

/// One registered GraphQL command mutation for client generators.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CommandCatalogEntry {
    pub command_name: String,
    pub field_name: String,
    pub roles: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<CommandTypeCatalog>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<CommandTypeCatalog>,
    /// Browser pipeline policy (result + reconcile). Omitted when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_reconcile: Option<ClientReconcile>,
}

/// Versioned command registry export (JSON).
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CommandCatalog {
    pub version: u32,
    pub commands: Vec<CommandCatalogEntry>,
}

fn type_catalog(def: &GraphqlTypeDef) -> CommandTypeCatalog {
    CommandTypeCatalog {
        name: def.name.clone(),
        fields: def.fields.iter().map(field_catalog).collect(),
    }
}

fn field_catalog(f: &GraphqlTypeField) -> CommandFieldCatalog {
    CommandFieldCatalog {
        name: f.name.clone(),
        type_name: f.type_name.clone(),
        nullable: f.nullable,
        list: f.list,
        item_nullable: f.item_nullable,
    }
}

fn surface_type(def: &GraphqlTypeDef) -> SurfaceTypeDef {
    SurfaceTypeDef {
        name: def.name.clone(),
        fields: def
            .fields
            .iter()
            .map(|field| SurfaceTypeField {
                name: field.name.clone(),
                type_name: field.type_name.clone(),
                nullable: field.nullable,
                list: field.list,
                item_nullable: field.item_nullable,
                nested: field.nested.as_deref().map(surface_type).map(Box::new),
            })
            .collect(),
    }
}

impl GraphqlCommands {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn command(mut self, name: &str, mut c: ExposedCommand) -> Self {
        if self.commands.iter().any(|(n, _)| n == name) {
            panic!("command `{name}` is already registered");
        }
        c.command_name = name.to_string();
        self.commands.push((name.to_string(), c));
        self
    }

    #[cfg_attr(not(feature = "graphql"), allow(dead_code))]
    pub(crate) fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.iter().map(|(n, _)| n.as_str())
    }

    pub(crate) fn surface_commands(&self) -> Vec<SurfaceCommand> {
        let mut commands: Vec<_> = self
            .commands
            .iter()
            .map(|(name, command)| {
                let mut roles = command.roles.clone();
                roles.sort();
                roles.dedup();
                SurfaceCommand {
                    command_name: name.clone(),
                    field_name: command.resolved_field_name(name),
                    roles,
                    input: match &command.input {
                        CommandInput::None => SurfaceCommandShape::None,
                        CommandInput::Json => SurfaceCommandShape::Json,
                        CommandInput::Typed(def) => SurfaceCommandShape::Typed(surface_type(def)),
                    },
                    output: match &command.output {
                        CommandOutput::Json => SurfaceCommandShape::Json,
                        CommandOutput::Typed(def) => SurfaceCommandShape::Typed(surface_type(def)),
                    },
                    consistency: command.consistency,
                    input_defaults: command.input_defaults.clone(),
                    effects: command.effects.clone(),
                    confirmations: command.confirmations.clone(),
                    projected_model: command.projected_model.clone(),
                    direct_projection: command.direct_projection.clone(),
                    confirmation_unavailable: command.confirmation_unavailable,
                }
            })
            .collect();
        commands.sort_by(|a, b| a.command_name.cmp(&b.command_name));
        commands
    }

    pub(crate) fn from_typed_contracts(contracts: &[TypedCommandContract]) -> Result<Self, String> {
        let mut commands = Self::new();
        for contract in contracts {
            if contract.name.trim().is_empty() {
                return Err("typed command id must not be empty".into());
            }
            let command = ExposedCommand {
                command_name: contract.name.clone(),
                field_name: Some(contract.field_name.clone()),
                input: CommandInput::Typed(contract.input.clone()),
                output: CommandOutput::Typed(contract.output.clone()),
                roles: contract.roles.clone(),
                client_reconcile: None,
                consistency: Some(contract.consistency),
                input_defaults: contract.input_defaults.clone(),
                effects: Some(contract.effects.clone()),
                confirmations: contract.confirmations.clone(),
                projected_model: contract.projected_model.clone(),
                direct_projection: contract.direct_projection.clone(),
                confirmation_unavailable: false,
            };
            commands = commands.command(&contract.name, command);
        }
        Ok(commands)
    }

    #[cfg(feature = "graphql")]
    pub(crate) fn typed_contracts_for_binding(&self) -> Result<Vec<TypedCommandContract>, String> {
        self.commands
            .iter()
            .map(|(name, command)| {
                let CommandInput::Typed(input) = &command.input else {
                    return Err(format!(
                        "bound typed command `{name}` no longer has a typed GraphQL input"
                    ));
                };
                let CommandOutput::Typed(output) = &command.output else {
                    return Err(format!(
                        "bound typed command `{name}` no longer has a typed GraphQL output"
                    ));
                };
                let input_type_id = input.type_id.ok_or_else(|| {
                    format!("bound typed command `{name}` input is missing its Rust TypeId")
                })?;
                let output_type_id = output.type_id.ok_or_else(|| {
                    format!("bound typed command `{name}` output is missing its Rust TypeId")
                })?;
                let consistency = command.consistency.ok_or_else(|| {
                    format!("bound typed command `{name}` is missing consistency metadata")
                })?;
                let mut effects = command.effects.clone().ok_or_else(|| {
                    format!("bound typed command `{name}` is missing effect metadata")
                })?;
                effects.canonicalize();
                Ok(TypedCommandContract {
                    name: name.clone(),
                    field_name: command.resolved_field_name(name),
                    roles: command.roles.clone(),
                    input: input.clone(),
                    output: output.clone(),
                    input_type_id,
                    output_type_id,
                    consistency,
                    input_defaults: command.input_defaults.clone(),
                    effects,
                    confirmations: command.confirmations.clone(),
                    projected_model: command.projected_model.clone(),
                    direct_projection: command.direct_projection.clone(),
                })
            })
            .collect()
    }

    /// Bind every confirmation and ordinary `Projected<M>` target to the exact
    /// compiled projector registry. The digest covers the full model schemas,
    /// accepted facts, and versioned scope codec; runtime lowering never
    /// reconstructs authority from projector/model strings.
    #[cfg(feature = "graphql")]
    pub(crate) fn bind_direct_projection_targets(
        &mut self,
        projectors: &[SurfaceProjector],
        model_schemas: &std::collections::BTreeMap<String, crate::table::TableSchema>,
    ) -> Result<(), String> {
        let mut compiled_projectors = std::collections::BTreeMap::new();
        for projector in projectors {
            let schemas = projector
                .models
                .iter()
                .map(|model| {
                    model_schemas.get(model).ok_or_else(|| {
                        format!(
                            "projector `{}` references unknown model `{model}`",
                            projector.name
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            let compiled = compile_projection_topology(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
                schemas,
            )
            .map_err(|error| {
                format!(
                    "projector `{}` has invalid compiled topology: {error}",
                    projector.name
                )
            })?;
            compiled_projectors.insert(projector.name.clone(), compiled);
        }

        for (name, command) in &mut self.commands {
            for confirmation in &mut command.confirmations {
                let projector = projectors
                    .iter()
                    .find(|projector| projector.name == confirmation.projector)
                    .ok_or_else(|| {
                        format!(
                            "typed command `{name}` expects unknown projector `{}`",
                            confirmation.projector
                        )
                    })?;
                if !confirmation.topology_matches(
                    &projector.name,
                    &projector.facts,
                    &projector.models,
                    &projector.partition,
                ) {
                    return Err(format!(
                        "typed command `{name}` captured projector `{}` topology identity does not match the registered projector facts/models",
                        confirmation.projector
                    ));
                }
                if !confirmation.partition_matches(&projector.partition) {
                    return Err(format!(
                        "typed command `{name}` confirmation for projector `{}` does not provide the partition mapping required by its declaration",
                        confirmation.projector
                    ));
                }
                if !projector
                    .models
                    .iter()
                    .any(|model| model == &confirmation.model)
                {
                    return Err(format!(
                        "typed command `{name}` expects projector `{}` to confirm model `{}`, but that model is not in the projector topology",
                        confirmation.projector, confirmation.model
                    ));
                }
                let (topology, _) = compiled_projectors
                    .get(&projector.name)
                    .expect("every registered projector was compiled above");
                confirmation.bind_protocol_topology(topology.clone());
            }

            match command.consistency {
                Some(CommandConsistency::Projected) => {}
                _ => continue,
            }
            let projected = command.projected_model.as_ref().ok_or_else(|| {
                format!(
                    "typed projected command `{name}` is missing its compiler-retained relational model"
                )
            })?;
            let owners = projectors
                .iter()
                .filter(|projector| {
                    projector
                        .models
                        .iter()
                        .any(|model| model == &projected.model)
                })
                .collect::<Vec<_>>();
            let projector = match owners.as_slice() {
                [projector] => *projector,
                [] => {
                    return Err(format!(
                        "typed projected command `{name}` output model `{}` has no registered SurfaceProjector owner",
                        projected.model
                    ))
                }
                _ => {
                    return Err(format!(
                        "typed projected command `{name}` output model `{}` has ambiguous SurfaceProjector ownership: {}",
                        projected.model,
                        owners
                            .iter()
                            .map(|owner| owner.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            };
            if projector.change_epoch.is_none() {
                return Err(format!(
                    "typed projected command `{name}` owner `{}` has no registered change-log epoch",
                    projector.name
                ));
            }
            let registered_schema = model_schemas
                .get(&projected.model)
                .expect("projector ownership above requires a registered model schema");
            if projected.schema != registered_schema {
                return Err(format!(
                    "typed projected command `{name}` retained schema for `{}` differs from the registered full table schema",
                    projected.model
                ));
            }
            if !projected.partition_matches(&projector.partition) {
                return Err(format!(
                    "typed projected command `{name}` does not provide the partition mapping required by projector `{}`",
                    projector.name
                ));
            }
            let (protocol_topology, ownership) = compiled_projectors
                .get(&projector.name)
                .expect("every registered projector was compiled above");
            command.direct_projection = Some(projected.bind(
                &projector.name,
                &projector.facts,
                &projector.models,
                &projector.partition,
                projector.change_epoch.as_deref(),
                ownership.clone(),
                Some(protocol_topology.clone()),
            ));
        }
        Ok(())
    }

    /// Machine-readable command registry for TypeScript (or other) client generators.
    pub fn catalog(&self) -> CommandCatalog {
        let commands = self
            .commands
            .iter()
            .map(|(name, cmd)| {
                let field_name = cmd.resolved_field_name(name);
                let input = match &cmd.input {
                    CommandInput::Typed(t) => Some(type_catalog(t)),
                    CommandInput::None | CommandInput::Json => None,
                };
                let output = match &cmd.output {
                    CommandOutput::Typed(t) => Some(type_catalog(t)),
                    CommandOutput::Json => None,
                };
                CommandCatalogEntry {
                    command_name: name.clone(),
                    field_name,
                    roles: cmd.roles.clone(),
                    input,
                    output,
                    client_reconcile: cmd.client_reconcile.clone(),
                }
            })
            .collect();
        CommandCatalog {
            version: 1,
            commands,
        }
    }

    /// Pretty JSON for `commands.manifest.json` artifacts.
    pub fn catalog_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.catalog())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphql::types::GraphqlTypeField;

    #[derive(Clone)]
    struct DummyIn;
    impl GraphqlInputType for DummyIn {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "DummyIn",
                vec![GraphqlTypeField {
                    name: "todo_id".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
        }
    }

    #[derive(Clone)]
    struct DummyOut;
    impl GraphqlOutputType for DummyOut {
        fn graphql_type() -> GraphqlTypeDef {
            GraphqlTypeDef::new(
                "DummyOut",
                vec![GraphqlTypeField {
                    name: "status".into(),
                    type_name: "String".into(),
                    nullable: false,
                    list: false,
                    item_nullable: false,
                    nested: None,
                }],
            )
        }
    }

    #[test]
    fn catalog_exports_field_roles_and_types() {
        let cmds = GraphqlCommands::new()
            .command(
                "todo.create",
                exposed_command()
                    .field_name("todos_create")
                    .input::<DummyIn>()
                    .output::<DummyOut>()
                    .roles(["user", "admin"])
                    .client_reconcile(ClientReconcile::fact()),
            )
            .command(
                "todo.force_archive",
                exposed_command()
                    .field_name("todos_force_archive")
                    .input::<DummyIn>()
                    .output::<DummyOut>()
                    .roles(["admin"]),
            )
            .command(
                "blob.move",
                exposed_command()
                    .field_name("blob_games_move")
                    .input::<DummyIn>()
                    .output::<DummyOut>()
                    .roles(["user"])
                    .client_reconcile(ClientReconcile::projection()),
            );

        let cat = cmds.catalog();
        assert_eq!(cat.version, 1);
        assert_eq!(cat.commands.len(), 3);
        assert_eq!(cat.commands[0].command_name, "todo.create");
        assert_eq!(cat.commands[0].field_name, "todos_create");
        assert_eq!(cat.commands[0].roles, vec!["user", "admin"]);
        assert_eq!(
            cat.commands[0].input.as_ref().unwrap().fields[0].name,
            "todo_id"
        );
        assert_eq!(
            cat.commands[0]
                .client_reconcile
                .as_ref()
                .unwrap()
                .result
                .kind,
            ClientResultKind::Fact
        );
        assert!(cat.commands[1].client_reconcile.is_none());
        assert_eq!(
            cat.commands[2]
                .client_reconcile
                .as_ref()
                .unwrap()
                .result
                .kind,
            ClientResultKind::Projection
        );

        let json = cmds.catalog_json_pretty().expect("json");
        assert!(json.contains("todos_create"));
        assert!(json.contains("\"admin\""));
        assert!(json.contains("todo_id"));
        assert!(json.contains("client_reconcile"));
        assert!(json.contains("\"projection\""));
        // Unset policy omitted (backward compatible).
        assert!(
            !json.contains("todos_force_archive") || {
                // force_archive entry must not carry client_reconcile
                let v: serde_json::Value = serde_json::from_str(&json).unwrap();
                let force = v["commands"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|c| c["field_name"] == "todos_force_archive")
                    .unwrap();
                force.get("client_reconcile").is_none()
            }
        );
    }
}
