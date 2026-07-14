//! Command mutations (Hasura-actions parity): typed GraphQL mutation fields
//! that dispatch through `CommandRequest` and never touch read-model tables.
//!
//! # Command client catalog
//!
//! [`GraphqlCommands::catalog`] exports a machine-readable registry used by
//! TypeScript generators so app code can call `todosCreate(input)` without
//! hand-authoring GraphQL mutation documents. GraphQL remains the wire.

use serde::Serialize;

use super::types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef, GraphqlTypeField};

#[derive(Clone, Debug)]
pub struct ExposedCommand {
    pub(crate) command_name: String,
    pub(crate) field_name: Option<String>,
    pub(crate) input: CommandInput,
    pub(crate) output: CommandOutput,
    pub(crate) roles: Vec<String>,
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

    pub(crate) fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.iter().map(|(n, _)| n.as_str())
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
                    .roles(["user", "admin"]),
            )
            .command(
                "todo.force_archive",
                exposed_command()
                    .field_name("todos_force_archive")
                    .input::<DummyIn>()
                    .output::<DummyOut>()
                    .roles(["admin"]),
            );

        let cat = cmds.catalog();
        assert_eq!(cat.version, 1);
        assert_eq!(cat.commands.len(), 2);
        assert_eq!(cat.commands[0].command_name, "todo.create");
        assert_eq!(cat.commands[0].field_name, "todos_create");
        assert_eq!(cat.commands[0].roles, vec!["user", "admin"]);
        assert_eq!(
            cat.commands[0].input.as_ref().unwrap().fields[0].name,
            "todo_id"
        );
        assert_eq!(cat.commands[1].roles, vec!["admin"]);

        let json = cmds.catalog_json_pretty().expect("json");
        assert!(json.contains("todos_create"));
        assert!(json.contains("\"admin\""));
        assert!(json.contains("todo_id"));
    }
}
