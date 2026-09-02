use super::super::graphql::{
    typescript_scalar, Cardinality, CompiledFilterInputField, CompiledFilterInputTarget,
    CompiledInputDefinition, CompiledInputType, CompiledMember, CompiledObject, CompiledOperation,
};
use super::super::ClientCompileError;
use super::common::{json_string, quoted_property};

pub(super) fn render_variables_type(
    operation: &CompiledOperation,
    name: &str,
) -> Result<String, ClientCompileError> {
    if operation.variables.is_empty() {
        return Ok(format!("export type {name} = Record<string, never>;"));
    }
    let mut blocks = operation
        .variable_codec
        .inputs
        .iter()
        .map(|(input_name, definition)| {
            render_input_definition(&operation.export_name, input_name, definition)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut lines = vec![format!("export type {name} = {{")];
    for variable in &operation.variables {
        let input_type = operation
            .variable_codec
            .variables
            .get(&variable.name)
            .ok_or_else(|| {
                ClientCompileError::manifest(
                    "client.render.variable_codec",
                    format!(
                        "operation `{}` has no codec for variable `${}`",
                        operation.name, variable.name
                    ),
                )
            })?;
        lines.push(format!(
            "  readonly {}{}: {};",
            quoted_property(&variable.name),
            if variable.graphql_type.nullable || variable.default.is_some() {
                "?"
            } else {
                ""
            },
            render_input_type(&operation.export_name, input_type)?
        ));
    }
    lines.push("};".into());
    blocks.push(lines.join("\n"));
    Ok(blocks.join("\n\n"))
}

fn render_input_definition(
    operation: &str,
    name: &str,
    definition: &CompiledInputDefinition,
) -> Result<String, ClientCompileError> {
    let alias = input_alias(operation, name);
    match definition {
        CompiledInputDefinition::Filter {
            fields,
            relationships,
            ..
        } => {
            let mut lines = vec![format!("type {alias} = {{")];
            for operator in ["_and", "_or"] {
                lines.push(format!(
                    "  readonly {}?: {alias} | readonly {alias}[] | null;",
                    quoted_property(operator)
                ));
            }
            lines.push(format!(
                "  readonly {}?: {alias} | null;",
                quoted_property("_not")
            ));
            for field in fields {
                lines.push(format!(
                    "  readonly {}?: {};",
                    quoted_property(&field.field),
                    render_filter_comparison(field)?
                ));
            }
            for relationship in relationships {
                let target = match &relationship.target {
                    CompiledFilterInputTarget::Input { name } => input_alias(operation, name),
                    CompiledFilterInputTarget::Opaque => {
                        "{ readonly [key: string]: ReplicaValue }".into()
                    }
                };
                lines.push(format!(
                    "  readonly {}?: {target} | null;",
                    quoted_property(&relationship.field)
                ));
            }
            lines.push("};".into());
            Ok(lines.join("\n"))
        }
        CompiledInputDefinition::Order { fields, values, .. } => {
            let direction = format!("{alias}_Direction");
            let direction_type = render_string_union(values)?;
            let mut lines = vec![format!("type {direction} = {direction_type};")];
            if fields.is_empty() {
                lines.push(format!("type {alias} = never;"));
            } else {
                lines.push(format!("type {alias} ="));
                for (index, field) in fields.iter().enumerate() {
                    lines.push("  | {".into());
                    for candidate in fields {
                        let property = quoted_property(&candidate.field);
                        if candidate.field == field.field {
                            lines.push(format!("      readonly {property}: {direction};"));
                        } else {
                            lines.push(format!("      readonly {property}?: never;"));
                        }
                    }
                    lines.push(if index + 1 == fields.len() {
                        "    };".into()
                    } else {
                        "    }".into()
                    });
                }
            }
            Ok(lines.join("\n"))
        }
    }
}

fn render_filter_comparison(
    field: &CompiledFilterInputField,
) -> Result<String, ClientCompileError> {
    let scalar = typescript_input_scalar(&field.scalar, &field.codec)?;
    let mut lines = vec!["{".to_string()];
    for operator in &field.operators {
        let value = match operator.as_str() {
            "_is_null" => "boolean | null".into(),
            "_in" | "_nin" => format!("{scalar} | readonly {}[]", parenthesize_ts_union(scalar)),
            "_has_key" => "string | null".into(),
            _ => format!("{scalar} | null"),
        };
        lines.push(format!(
            "    readonly {}?: {value};",
            quoted_property(operator)
        ));
    }
    lines.push("  } | null".into());
    Ok(lines.join("\n"))
}

fn render_input_type(
    operation: &str,
    input_type: &CompiledInputType,
) -> Result<String, ClientCompileError> {
    let (base, nullable) = match input_type {
        CompiledInputType::Scalar {
            scalar,
            codec,
            nullable,
        } => (
            typescript_input_scalar(scalar, codec)?.to_string(),
            *nullable,
        ),
        CompiledInputType::Enum {
            values, nullable, ..
        } => (render_string_union(values)?, *nullable),
        CompiledInputType::Input { name, nullable, .. } => {
            (input_alias(operation, name), *nullable)
        }
        CompiledInputType::List { nullable, item, .. } => {
            let item = render_input_type(operation, item)?;
            (
                format!("{item} | readonly {}[]", parenthesize_ts_union(&item)),
                *nullable,
            )
        }
    };
    Ok(if nullable {
        format!("{base} | null")
    } else {
        base
    })
}

fn typescript_input_scalar(scalar: &str, codec: &str) -> Result<&'static str, ClientCompileError> {
    match (scalar, codec) {
        ("ID", "string") => Ok("string | number"),
        ("String", "string")
        | ("Bytea", "base64")
        | ("Timestamptz", "string_unvalidated_timestamp") => Ok("string"),
        ("Boolean", "boolean") => Ok("boolean"),
        ("Int", "int32") | ("Float", "float64") | ("BigInt", "json_number_precision_limited") => {
            Ok("number")
        }
        ("JSON", "json") => Ok("ReplicaValue"),
        _ => Err(ClientCompileError::manifest(
            "client.scalar.codec_unsupported",
            format!("scalar `{scalar}` uses unsupported input codec `{codec}`"),
        )),
    }
}

fn render_string_union(values: &[String]) -> Result<String, ClientCompileError> {
    if values.is_empty() {
        return Err(ClientCompileError::manifest(
            "client.render.input_enum",
            "generated input enum must contain at least one value",
        ));
    }
    values
        .iter()
        .map(|value| json_string(value))
        .collect::<Result<Vec<_>, _>>()
        .map(|values| values.join(" | "))
}

fn input_alias(operation: &str, input: &str) -> String {
    format!("{operation}_Input_{input}")
}

fn parenthesize_ts_union(value: &str) -> String {
    if value.contains(" | ") {
        format!("({value})")
    } else {
        value.to_string()
    }
}

pub(super) fn render_data_type(
    operation: &CompiledOperation,
    name: &str,
) -> Result<String, ClientCompileError> {
    let root = &operation.root;
    let entity = render_object_type(&root.selection, 2)?;
    let value = match root.cardinality {
        Cardinality::Many => format!("readonly {entity}[]"),
        Cardinality::One => {
            if root.nullable {
                format!("{entity} | null")
            } else {
                entity
            }
        }
    };
    Ok(format!(
        "export type {name} = {{\n  readonly {}: {};\n}};",
        quoted_property(&root.response_key),
        value
    ))
}

fn render_object_type(
    object: &CompiledObject,
    indent: usize,
) -> Result<String, ClientCompileError> {
    let member_padding = " ".repeat(indent + 2);
    let closing_padding = " ".repeat(indent);
    let mut lines = vec!["{".to_string()];
    for member in &object.members {
        match member {
            CompiledMember::Scalar(field) if field.expose => {
                let scalar = typescript_scalar(field)?;
                lines.push(format!(
                    "{member_padding}readonly {}: {}{};",
                    quoted_property(&field.response_key),
                    scalar,
                    if field.nullable { " | null" } else { "" }
                ));
            }
            CompiledMember::Scalar(_) => {}
            CompiledMember::Branch(branch) => {
                let object = render_object_type(&branch.selection, indent + 2)?;
                let value = match branch.cardinality {
                    Cardinality::Many => format!("readonly {object}[]"),
                    Cardinality::One if branch.nullable => format!("{object} | null"),
                    Cardinality::One => object,
                };
                lines.push(format!(
                    "{member_padding}readonly {}: {value};",
                    quoted_property(&branch.response_key)
                ));
            }
        }
    }
    lines.push(format!("{closing_padding}}}"));
    Ok(lines.join("\n"))
}
