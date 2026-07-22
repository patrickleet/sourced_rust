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

use super::surface::{SurfaceCommand, SurfaceCommandShape, SurfaceTypeDef, SurfaceTypeField};
use super::types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};

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
                }
            })
            .collect();
        commands.sort_by(|a, b| a.command_name.cmp(&b.command_name));
        commands
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
