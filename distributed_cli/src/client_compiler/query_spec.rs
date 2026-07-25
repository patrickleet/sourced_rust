//! Portable QuerySpec IR — a second authoring surface for client operations.
//!
//! GraphQL documents and QuerySpec files are dual frontends. Both lower into
//! the same GraphQL operation text before the existing compiler path runs, so
//! artifact hashes, route discovery, and fail-closed validation stay singular.
//!
//! QuerySpec is intentionally small:
//! - one named query operation
//! - optional `@load` / `@live`
//! - closed variable declarations
//! - root field selection with literal or variable arguments
//! - nested field selections for relationship includes
//!
//! Enum values and variables use tagged JSON forms so lowering is unambiguous:
//! - `{ "$enum": "asc" }` → GraphQL enum `asc`
//! - `{ "$var": "status" }` → GraphQL variable `$status`

use serde::Deserialize;
use serde_json::Value as JsonValue;

use super::{ClientCompileError, ClientDocument};

const QUERY_SPEC_VERSION: u32 = 1;
const MAX_SELECTION_DEPTH: usize = 16;
const MAX_SELECTION_FIELDS: usize = 512;

/// True when the path is a QuerySpec source (not a GraphQL document).
pub(crate) fn is_query_spec_path(path: &str) -> bool {
    path.ends_with(".query.json")
}

/// If `document` is a QuerySpec file, replace its source with lowered GraphQL.
pub(crate) fn materialize_client_document(
    document: &mut ClientDocument,
) -> Result<(), ClientCompileError> {
    if !is_query_spec_path(&document.path) {
        return Ok(());
    }
    let spec = parse_query_spec(&document.path, &document.source)?;
    document.source = lower_query_spec(&document.path, &spec)?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuerySpec {
    version: u32,
    name: String,
    #[serde(default)]
    load: bool,
    #[serde(default)]
    live: bool,
    #[serde(default)]
    variables: Vec<QuerySpecVariable>,
    roots: Vec<QuerySpecRoot>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuerySpecVariable {
    name: String,
    #[serde(rename = "type")]
    graphql_type: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuerySpecRoot {
    field: String,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    args: JsonValue,
    select: JsonValue,
}

fn parse_query_spec(path: &str, source: &str) -> Result<QuerySpec, ClientCompileError> {
    let spec: QuerySpec = serde_json::from_str(source).map_err(|error| {
        ClientCompileError::source(
            "client.query_spec.invalid_json",
            format!("invalid QuerySpec JSON: {error}"),
            path,
            error.line(),
            error.column(),
        )
    })?;
    if spec.version != QUERY_SPEC_VERSION {
        return Err(ClientCompileError::source(
            "client.query_spec.unsupported_version",
            format!(
                "QuerySpec version {} is unsupported; expected {QUERY_SPEC_VERSION}",
                spec.version
            ),
            path,
            1,
            1,
        ));
    }
    validate_operation_name(path, &spec.name)?;
    if spec.roots.is_empty() {
        return Err(ClientCompileError::source(
            "client.query_spec.empty_roots",
            "QuerySpec requires at least one root field",
            path,
            1,
            1,
        ));
    }
    if spec.roots.len() != 1 {
        return Err(ClientCompileError::source(
            "client.query_spec.multiple_roots",
            "QuerySpec v1 supports exactly one root field (matches the GraphQL client compiler)",
            path,
            1,
            1,
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    for variable in &spec.variables {
        validate_ident(path, "variable", &variable.name)?;
        validate_graphql_type(path, &variable.graphql_type)?;
        if !names.insert(variable.name.as_str()) {
            return Err(ClientCompileError::source(
                "client.query_spec.duplicate_variable",
                format!("duplicate QuerySpec variable `{}`", variable.name),
                path,
                1,
                1,
            ));
        }
    }
    Ok(spec)
}

fn lower_query_spec(path: &str, spec: &QuerySpec) -> Result<String, ClientCompileError> {
    let mut directives = String::new();
    if spec.load {
        directives.push_str(" @load");
    }
    if spec.live {
        directives.push_str(" @live");
    }

    let variable_definitions = if spec.variables.is_empty() {
        String::new()
    } else {
        let parts = spec
            .variables
            .iter()
            .map(|variable| format!("${}: {}", variable.name, variable.graphql_type))
            .collect::<Vec<_>>();
        format!("({})", parts.join(", "))
    };

    let root = &spec.roots[0];
    validate_ident(path, "root field", &root.field)?;
    if let Some(alias) = &root.alias {
        validate_ident(path, "root alias", alias)?;
    }

    let head = match &root.alias {
        Some(alias) if alias != &root.field => format!("{alias}: {}", root.field),
        _ => root.field.clone(),
    };
    let args = render_arguments(path, &root.args)?;
    // Root field is indented 2 spaces; its selection members are one level deeper.
    let selection = render_selection(path, &root.select, 2, 0)?;

    // Keep the lowered document readable and stable for human diffing.
    Ok(format!(
        "query {}{}{} {{\n  {}{} {{\n{}  }}\n}}\n",
        spec.name, variable_definitions, directives, head, args, selection
    ))
}

fn render_arguments(path: &str, value: &JsonValue) -> Result<String, ClientCompileError> {
    match value {
        JsonValue::Null => Ok(String::new()),
        JsonValue::Object(map) if map.is_empty() => Ok(String::new()),
        JsonValue::Object(map) => {
            let mut parts = Vec::with_capacity(map.len());
            for (name, argument) in map {
                validate_ident(path, "argument", name)?;
                parts.push(format!(
                    "{name}: {}",
                    render_value(path, argument, ValueContext::Argument)?
                ));
            }
            Ok(format!("({})", parts.join(", ")))
        }
        _ => Err(ClientCompileError::source(
            "client.query_spec.invalid_args",
            "QuerySpec root `args` must be a JSON object",
            path,
            1,
            1,
        )),
    }
}

fn render_selection(
    path: &str,
    value: &JsonValue,
    indent: usize,
    depth: usize,
) -> Result<String, ClientCompileError> {
    if depth > MAX_SELECTION_DEPTH {
        return Err(ClientCompileError::source(
            "client.query_spec.selection_too_deep",
            format!("QuerySpec selection exceeds max depth {MAX_SELECTION_DEPTH}"),
            path,
            1,
            1,
        ));
    }
    let JsonValue::Object(map) = value else {
        return Err(ClientCompileError::source(
            "client.query_spec.invalid_select",
            "QuerySpec `select` must be an object of field → true | nested select",
            path,
            1,
            1,
        ));
    };
    if map.is_empty() {
        return Err(ClientCompileError::source(
            "client.query_spec.empty_select",
            "QuerySpec selection must include at least one field",
            path,
            1,
            1,
        ));
    }
    if map.len() > MAX_SELECTION_FIELDS {
        return Err(ClientCompileError::source(
            "client.query_spec.selection_too_wide",
            format!("QuerySpec selection exceeds max fields {MAX_SELECTION_FIELDS}"),
            path,
            1,
            1,
        ));
    }

    let pad = "  ".repeat(indent);
    let mut lines = Vec::with_capacity(map.len());
    for (field, selection) in map {
        validate_ident(path, "selection field", field)?;
        match selection {
            JsonValue::Bool(true) => lines.push(format!("{pad}{field}")),
            JsonValue::Bool(false) => {
                return Err(ClientCompileError::source(
                    "client.query_spec.field_excluded",
                    format!(
                        "QuerySpec field `{field}` is false; omit excluded fields instead of setting false"
                    ),
                    path,
                    1,
                    1,
                ));
            }
            JsonValue::Object(_) => {
                let nested = render_selection(path, selection, indent + 1, depth + 1)?;
                lines.push(format!("{pad}{field} {{\n{nested}{pad}}}"));
            }
            _ => {
                return Err(ClientCompileError::source(
                    "client.query_spec.invalid_field_selection",
                    format!(
                        "QuerySpec field `{field}` must be true or a nested selection object"
                    ),
                    path,
                    1,
                    1,
                ));
            }
        }
    }
    Ok(lines
        .into_iter()
        .map(|line| format!("{line}\n"))
        .collect())
}

#[derive(Clone, Copy)]
enum ValueContext {
    Argument,
}

fn render_value(
    path: &str,
    value: &JsonValue,
    _context: ValueContext,
) -> Result<String, ClientCompileError> {
    match value {
        JsonValue::Null => Ok("null".into()),
        JsonValue::Bool(flag) => Ok(if *flag { "true" } else { "false" }.into()),
        JsonValue::Number(number) => Ok(number.to_string()),
        JsonValue::String(text) => Ok(format!("\"{}\"", escape_graphql_string(text))),
        JsonValue::Array(items) => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                parts.push(render_value(path, item, ValueContext::Argument)?);
            }
            Ok(format!("[{}]", parts.join(", ")))
        }
        JsonValue::Object(map) => {
            if map.len() == 1 {
                if let Some(name) = map.get("$enum").and_then(JsonValue::as_str) {
                    validate_ident(path, "enum value", name)?;
                    return Ok(name.to_string());
                }
                if let Some(name) = map.get("$var").and_then(JsonValue::as_str) {
                    validate_ident(path, "variable reference", name)?;
                    return Ok(format!("${name}"));
                }
            }
            let mut parts = Vec::with_capacity(map.len());
            for (key, nested) in map {
                if key.starts_with('$') {
                    return Err(ClientCompileError::source(
                        "client.query_spec.unknown_tag",
                        format!(
                            "unknown QuerySpec value tag `{key}`; supported tags are `$enum` and `$var`"
                        ),
                        path,
                        1,
                        1,
                    ));
                }
                validate_ident(path, "input field", key)?;
                parts.push(format!(
                    "{key}: {}",
                    render_value(path, nested, ValueContext::Argument)?
                ));
            }
            Ok(format!("{{{}}}", parts.join(", ")))
        }
    }
}

fn escape_graphql_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn validate_operation_name(path: &str, name: &str) -> Result<(), ClientCompileError> {
    if name.is_empty() {
        return Err(ClientCompileError::source(
            "client.query_spec.empty_name",
            "QuerySpec `name` must be a non-empty GraphQL operation name",
            path,
            1,
            1,
        ));
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(ClientCompileError::source(
            "client.query_spec.empty_name",
            "QuerySpec `name` must be a non-empty GraphQL operation name",
            path,
            1,
            1,
        ));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(ClientCompileError::source(
            "client.query_spec.invalid_name",
            format!("QuerySpec `name` `{name}` is not a valid GraphQL name"),
            path,
            1,
            1,
        ));
    }
    Ok(())
}

fn validate_ident(path: &str, kind: &str, name: &str) -> Result<(), ClientCompileError> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return Err(ClientCompileError::source(
            "client.query_spec.invalid_ident",
            format!("QuerySpec {kind} must be a non-empty GraphQL name"),
            path,
            1,
            1,
        ));
    };
    if !(first.is_ascii_alphabetic() || first == '_')
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return Err(ClientCompileError::source(
            "client.query_spec.invalid_ident",
            format!("QuerySpec {kind} `{name}` is not a valid GraphQL name"),
            path,
            1,
            1,
        ));
    }
    Ok(())
}

fn validate_graphql_type(path: &str, graphql_type: &str) -> Result<(), ClientCompileError> {
    // Closed character set for variable types: Name, list, non-null, whitespace.
    if graphql_type.trim().is_empty() {
        return Err(ClientCompileError::source(
            "client.query_spec.invalid_type",
            "QuerySpec variable type must be non-empty",
            path,
            1,
            1,
        ));
    }
    if !graphql_type
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '!' | '[' | ']' | ' '))
    {
        return Err(ClientCompileError::source(
            "client.query_spec.invalid_type",
            format!("QuerySpec variable type `{graphql_type}` contains unsupported characters"),
            path,
            1,
            1,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_compiler::ClientDocument;

    #[test]
    fn lowers_todos_list_spec() {
        let mut document = ClientDocument::new(
            "src/routes/todos/+page.query.json",
            r#"{
              "version": 1,
              "name": "Todos",
              "load": true,
              "roots": [{
                "field": "todos",
                "args": {
                  "order_by": [
                    {"status": {"$enum": "asc"}},
                    {"todo_id": {"$enum": "asc"}}
                  ]
                },
                "select": {
                  "todo_id": true,
                  "owner_id": true,
                  "title": true,
                  "status": true
                }
              }]
            }"#,
        );
        materialize_client_document(&mut document).unwrap();
        assert_eq!(
            document.source,
            "query Todos @load {\n  todos(order_by: [{status: asc}, {todo_id: asc}]) {\n    todo_id\n    owner_id\n    title\n    status\n  }\n}\n"
        );
    }

    #[test]
    fn lowers_nested_select_and_live() {
        let mut document = ClientDocument::new(
            "src/routes/chat/+page.query.json",
            r#"{
              "version": 1,
              "name": "ChatMessages",
              "load": true,
              "live": true,
              "roots": [{
                "field": "chat_messages",
                "args": {
                  "order_by": [{"created_at": {"$enum": "asc"}}]
                },
                "select": {
                  "message_id": true,
                  "body": true,
                  "author": {
                    "display_name": true
                  }
                }
              }]
            }"#,
        );
        materialize_client_document(&mut document).unwrap();
        assert!(document.source.starts_with("query ChatMessages @load @live {"));
        assert!(document.source.contains("author {\n      display_name\n    }"));
    }

    #[test]
    fn lowers_variable_reference() {
        let mut document = ClientDocument::new(
            "src/lib/ops/todos-by-status.query.json",
            r#"{
              "version": 1,
              "name": "TodosByStatus",
              "variables": [{"name": "status", "type": "String!"}],
              "roots": [{
                "field": "todos",
                "args": {
                  "where": {"status": {"_eq": {"$var": "status"}}}
                },
                "select": {"todo_id": true, "status": true}
              }]
            }"#,
        );
        materialize_client_document(&mut document).unwrap();
        assert!(document
            .source
            .starts_with("query TodosByStatus($status: String!) {"));
        assert!(document.source.contains("where: {status: {_eq: $status}}"));
    }

    #[test]
    fn rejects_unknown_version() {
        let mut document = ClientDocument::new(
            "x.query.json",
            r#"{"version": 2, "name": "X", "roots": [{"field": "todos", "select": {"id": true}}]}"#,
        );
        let error = materialize_client_document(&mut document).unwrap_err();
        assert_eq!(error.code, "client.query_spec.unsupported_version");
    }

    #[test]
    fn leaves_graphql_documents_untouched() {
        let mut document = ClientDocument::new(
            "src/routes/todos/+page.graphql",
            "query Todos { todos { id } }\n",
        );
        materialize_client_document(&mut document).unwrap();
        assert_eq!(document.source, "query Todos { todos { id } }\n");
    }
}
