//! Command mutations (Hasura-actions parity): typed GraphQL mutation fields
//! that dispatch through `CommandRequest` and never touch read-model tables.

use super::types::{GraphqlInputType, GraphqlOutputType, GraphqlTypeDef};

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

impl GraphqlCommands {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn command(mut self, name: &str, mut c: ExposedCommand) -> Self {
        c.command_name = name.to_string();
        self.commands.push((name.to_string(), c));
        self
    }

    pub(crate) fn command_names(&self) -> impl Iterator<Item = &str> {
        self.commands.iter().map(|(n, _)| n.as_str())
    }
}
