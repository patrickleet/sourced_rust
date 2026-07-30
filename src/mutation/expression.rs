//! Event-independent mutation expressions over typed inputs.

use serde::Serialize;
use serde_json::Value;

use crate::projection::{
    ProjectionAssignment, ProjectionExpression, ProjectionScalarTransform, ProjectionValue,
    ProjectionValueRef, ProjectionValueType, MAX_PROJECTION_EXPRESSION_DEPTH,
    MAX_PROJECTION_PATH_SEGMENTS,
};

use super::MutationProgramError;

/// Maximum nesting of a mutation expression or literal value.
pub const MAX_MUTATION_EXPRESSION_DEPTH: usize = MAX_PROJECTION_EXPRESSION_DEPTH;

/// Maximum number of segments in a mutation input path.
pub const MAX_MUTATION_PATH_SEGMENTS: usize = MAX_PROJECTION_PATH_SEGMENTS;

/// One ordered object member in a mutation expression.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MutationExpressionObjectField {
    name: String,
    value: MutationExpression,
}

impl MutationExpressionObjectField {
    /// Return the member name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the member expression.
    pub fn value(&self) -> &MutationExpression {
        &self.value
    }
}

/// A bounded, deterministic expression over a bound mutation input object.
///
/// Mutation expressions never read domain events. Portable handlers bind event
/// fields into an input object before evaluation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct MutationExpression {
    expression: MutationExpressionKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum MutationExpressionKind {
    InputPath {
        path: Vec<String>,
        value_type: ProjectionValueType,
    },
    Constant {
        value: ProjectionValue,
    },
    Enum {
        enum_type: String,
        variant: String,
    },
    List {
        values: Vec<MutationExpression>,
    },
    Object {
        fields: Vec<MutationExpressionObjectField>,
    },
    Transform {
        transform: ProjectionScalarTransform,
        arguments: Vec<MutationExpression>,
    },
}

/// Field assignment that preserves null versus removal versus unknown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "expression", rename_all = "snake_case")]
pub enum MutationAssignment {
    /// Evaluate and assign a concrete value (including explicit null).
    Set(MutationExpression),
    /// Explicitly remove the field when the model supports unset.
    Unset,
    /// Field is omitted / unknown in a partial patch input.
    Unknown,
}

/// Result of evaluating a mutation expression against a bound input object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum ResolvedMutationValue {
    /// A concrete value; JSON null remains a concrete value.
    Value(ProjectionValue),
    /// The selected optional input did not exist.
    Absent,
    /// The mutation explicitly removes this field.
    Unset,
    /// The field was authored as unknown and must not be written.
    Unknown,
}

impl MutationExpression {
    /// Select a nested property from the bound mutation input.
    ///
    /// # Errors
    ///
    /// Rejects empty path segments and paths beyond the portable limit.
    pub fn input_path<I, S>(
        value_type: ProjectionValueType,
        segments: I,
    ) -> Result<Self, MutationProgramError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let path = segments.into_iter().map(Into::into).collect::<Vec<_>>();
        if path.len() > MAX_MUTATION_PATH_SEGMENTS {
            return Err(MutationProgramError::PathTooDeep {
                segments: path.len(),
                max: MAX_MUTATION_PATH_SEGMENTS,
            });
        }
        if path.iter().any(|segment| segment.is_empty()) {
            return Err(MutationProgramError::EmptyName("input path segment"));
        }
        if matches!(&value_type, ProjectionValueType::Enum(name) if name.is_empty()) {
            return Err(MutationProgramError::EmptyName("input enum type"));
        }
        Ok(Self {
            expression: MutationExpressionKind::InputPath { path, value_type },
        })
    }

    /// Embed a validated literal.
    pub fn constant(value: ProjectionValue) -> Self {
        Self {
            expression: MutationExpressionKind::Constant { value },
        }
    }

    /// Embed a typed enum variant.
    ///
    /// # Errors
    ///
    /// Rejects empty enum type or variant names.
    pub fn enum_variant(
        enum_type: impl Into<String>,
        variant: impl Into<String>,
    ) -> Result<Self, MutationProgramError> {
        let enum_type = non_empty(enum_type.into(), "enum type")?;
        let variant = non_empty(variant.into(), "enum variant")?;
        Ok(Self {
            expression: MutationExpressionKind::Enum { enum_type, variant },
        })
    }

    /// Construct a deterministic list from child expressions.
    ///
    /// # Errors
    ///
    /// Rejects expression trees beyond the depth limit.
    pub fn list(values: Vec<Self>) -> Result<Self, MutationProgramError> {
        let expression = Self {
            expression: MutationExpressionKind::List { values },
        };
        expression.validate_depth()?;
        Ok(expression)
    }

    /// Construct an object whose keys are canonicalized lexicographically.
    ///
    /// # Errors
    ///
    /// Rejects empty or duplicate fields and trees beyond the depth limit.
    pub fn object<I, S>(fields: I) -> Result<Self, MutationProgramError>
    where
        I: IntoIterator<Item = (S, Self)>,
        S: Into<String>,
    {
        let mut fields = fields
            .into_iter()
            .map(|(name, value)| {
                Ok(MutationExpressionObjectField {
                    name: non_empty(name.into(), "object field")?,
                    value,
                })
            })
            .collect::<Result<Vec<_>, MutationProgramError>>()?;
        fields.sort_by(|left, right| left.name.cmp(&right.name));
        for pair in fields.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(MutationProgramError::DuplicateName {
                    kind: "object field",
                    name: pair[0].name.clone(),
                });
            }
        }
        let expression = Self {
            expression: MutationExpressionKind::Object { fields },
        };
        expression.validate_depth()?;
        Ok(expression)
    }

    /// Apply one closed, versioned scalar transform.
    ///
    /// # Errors
    ///
    /// Rejects an empty argument list and trees beyond the depth limit.
    pub fn transform(
        transform: ProjectionScalarTransform,
        arguments: Vec<Self>,
    ) -> Result<Self, MutationProgramError> {
        if arguments.is_empty() {
            return Err(MutationProgramError::InvalidOperation {
                operation: "mutation expression".to_owned(),
                reason: "scalar transforms require at least one argument".to_owned(),
            });
        }
        let expression = Self {
            expression: MutationExpressionKind::Transform {
                transform,
                arguments,
            },
        };
        expression.validate_depth()?;
        Ok(expression)
    }

    /// Return whether this expression is a pure constant (no input reads).
    pub fn is_constant(&self) -> bool {
        match &self.expression {
            MutationExpressionKind::Constant { .. } | MutationExpressionKind::Enum { .. } => true,
            MutationExpressionKind::InputPath { .. } => false,
            MutationExpressionKind::List { values } => values.iter().all(Self::is_constant),
            MutationExpressionKind::Object { fields } => {
                fields.iter().all(|field| field.value.is_constant())
            }
            MutationExpressionKind::Transform { arguments, .. } => {
                arguments.iter().all(Self::is_constant)
            }
        }
    }

    /// Borrow the input path segments when this is an input path.
    pub fn as_input_path(&self) -> Option<(&[String], &ProjectionValueType)> {
        match &self.expression {
            MutationExpressionKind::InputPath { path, value_type } => {
                Some((path.as_slice(), value_type))
            }
            _ => None,
        }
    }

    /// Evaluate against a bound JSON input object.
    ///
    /// # Errors
    ///
    /// Returns typed path/type mismatches for unsupported shapes.
    pub fn resolve(&self, input: &Value) -> Result<ResolvedMutationValue, MutationProgramError> {
        match &self.expression {
            MutationExpressionKind::InputPath { path, value_type } => {
                let mut current = input;
                for segment in path {
                    let Value::Object(object) = current else {
                        return Ok(ResolvedMutationValue::Absent);
                    };
                    let Some(next) = object.get(segment) else {
                        return Ok(ResolvedMutationValue::Absent);
                    };
                    current = next;
                }
                let _ = value_type;
                Ok(ResolvedMutationValue::Value(
                    ProjectionValue::try_from_json(current.clone()).map_err(|error| {
                        MutationProgramError::Adapter(format!(
                            "typed input path `{}`: {error}",
                            path.join(".")
                        ))
                    })?,
                ))
            }
            MutationExpressionKind::Constant { value } => {
                Ok(ResolvedMutationValue::Value(value.clone()))
            }
            MutationExpressionKind::Enum { enum_type, variant } => {
                ProjectionExpression::enum_variant(enum_type, variant)
                    .map_err(MutationProgramError::from)?;
                Ok(ResolvedMutationValue::Value(ProjectionValue::string(
                    variant.clone(),
                )))
            }
            MutationExpressionKind::List { values } => {
                let mut items = Vec::with_capacity(values.len());
                for value in values {
                    match value.resolve(input)? {
                        ResolvedMutationValue::Value(resolved) => {
                            items.push(projection_value_to_json(&resolved)?);
                        }
                        ResolvedMutationValue::Absent => {
                            return Ok(ResolvedMutationValue::Absent);
                        }
                        ResolvedMutationValue::Unset | ResolvedMutationValue::Unknown => {
                            return Err(MutationProgramError::InvalidOperation {
                                operation: "list expression".to_owned(),
                                reason: "list members cannot be unset or unknown".to_owned(),
                            });
                        }
                    }
                }
                Ok(ResolvedMutationValue::Value(
                    ProjectionValue::try_from_json(Value::Array(items))
                        .map_err(|error| MutationProgramError::Adapter(error.to_string()))?,
                ))
            }
            MutationExpressionKind::Object { fields } => {
                let mut object = serde_json::Map::new();
                for field in fields {
                    match field.value.resolve(input)? {
                        ResolvedMutationValue::Value(resolved) => {
                            object.insert(field.name.clone(), projection_value_to_json(&resolved)?);
                        }
                        ResolvedMutationValue::Absent => {
                            return Ok(ResolvedMutationValue::Absent);
                        }
                        ResolvedMutationValue::Unset | ResolvedMutationValue::Unknown => {
                            return Err(MutationProgramError::InvalidOperation {
                                operation: "object expression".to_owned(),
                                reason: "object members cannot be unset or unknown".to_owned(),
                            });
                        }
                    }
                }
                Ok(ResolvedMutationValue::Value(
                    ProjectionValue::try_from_json(Value::Object(object))
                        .map_err(|error| MutationProgramError::Adapter(error.to_string()))?,
                ))
            }
            MutationExpressionKind::Transform { .. } => Err(MutationProgramError::Adapter(
                "transform evaluation requires projection rewrite".to_owned(),
            )),
        }
    }

    /// Rewrite this mutation expression into a projection expression using a
    /// binder that maps input roots onto event/envelope/constant expressions.
    ///
    /// # Errors
    ///
    /// Returns adapter failures when a required input binding is missing.
    pub fn rewrite_with(
        &self,
        bind_input_path: &dyn Fn(
            &[String],
            &ProjectionValueType,
        ) -> Result<ProjectionExpression, MutationProgramError>,
    ) -> Result<ProjectionExpression, MutationProgramError> {
        match &self.expression {
            MutationExpressionKind::InputPath { path, value_type } => {
                bind_input_path(path, value_type)
            }
            MutationExpressionKind::Constant { value } => {
                Ok(ProjectionExpression::constant(value.clone()))
            }
            MutationExpressionKind::Enum { enum_type, variant } => {
                ProjectionExpression::enum_variant(enum_type, variant).map_err(Into::into)
            }
            MutationExpressionKind::List { values } => {
                let rewritten = values
                    .iter()
                    .map(|value| value.rewrite_with(bind_input_path))
                    .collect::<Result<Vec<_>, _>>()?;
                ProjectionExpression::list(rewritten).map_err(Into::into)
            }
            MutationExpressionKind::Object { fields } => {
                let rewritten = fields
                    .iter()
                    .map(|field| {
                        Ok((
                            field.name.clone(),
                            field.value.rewrite_with(bind_input_path)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, MutationProgramError>>()?;
                ProjectionExpression::object(rewritten).map_err(Into::into)
            }
            MutationExpressionKind::Transform {
                transform,
                arguments,
            } => {
                let rewritten = arguments
                    .iter()
                    .map(|argument| argument.rewrite_with(bind_input_path))
                    .collect::<Result<Vec<_>, _>>()?;
                ProjectionExpression::transform(*transform, rewritten).map_err(Into::into)
            }
        }
    }

    fn validate_depth(&self) -> Result<(), MutationProgramError> {
        let depth = expression_depth(self, 1);
        if depth > MAX_MUTATION_EXPRESSION_DEPTH {
            return Err(MutationProgramError::ExpressionTooDeep {
                depth,
                max: MAX_MUTATION_EXPRESSION_DEPTH,
            });
        }
        Ok(())
    }
}

impl MutationAssignment {
    /// Construct a set assignment.
    pub fn set(expression: MutationExpression) -> Self {
        Self::Set(expression)
    }

    /// Construct an explicit unset assignment.
    pub fn unset() -> Self {
        Self::Unset
    }

    /// Construct an unknown/omitted assignment.
    pub fn unknown() -> Self {
        Self::Unknown
    }

    /// Rewrite into a projection assignment. Unknown becomes omitted (caller filters).
    ///
    /// # Errors
    ///
    /// Propagates rewrite failures from nested expressions.
    pub fn rewrite_with(
        &self,
        bind_input_path: &dyn Fn(
            &[String],
            &ProjectionValueType,
        ) -> Result<ProjectionExpression, MutationProgramError>,
    ) -> Result<Option<ProjectionAssignment>, MutationProgramError> {
        match self {
            Self::Set(expression) => Ok(Some(ProjectionAssignment::Set(
                expression.rewrite_with(bind_input_path)?,
            ))),
            Self::Unset => Ok(Some(ProjectionAssignment::Unset)),
            Self::Unknown => Ok(None),
        }
    }
}

fn expression_depth(expression: &MutationExpression, level: usize) -> usize {
    match &expression.expression {
        MutationExpressionKind::InputPath { .. }
        | MutationExpressionKind::Constant { .. }
        | MutationExpressionKind::Enum { .. } => level,
        MutationExpressionKind::List { values } => values
            .iter()
            .map(|value| expression_depth(value, level + 1))
            .max()
            .unwrap_or(level),
        MutationExpressionKind::Object { fields } => fields
            .iter()
            .map(|field| expression_depth(&field.value, level + 1))
            .max()
            .unwrap_or(level),
        MutationExpressionKind::Transform { arguments, .. } => arguments
            .iter()
            .map(|argument| expression_depth(argument, level + 1))
            .max()
            .unwrap_or(level),
    }
}

pub(crate) fn non_empty(value: String, kind: &'static str) -> Result<String, MutationProgramError> {
    if value.is_empty() {
        Err(MutationProgramError::EmptyName(kind))
    } else {
        Ok(value)
    }
}

fn projection_value_to_json(value: &ProjectionValue) -> Result<Value, MutationProgramError> {
    Ok(match value.as_ref() {
        ProjectionValueRef::Null => Value::Null,
        ProjectionValueRef::Boolean(value) => Value::Bool(value),
        ProjectionValueRef::I64(text) => Value::String(text.to_owned()),
        ProjectionValueRef::U64(text) => Value::String(text.to_owned()),
        ProjectionValueRef::F64(text) => Value::String(text.to_owned()),
        ProjectionValueRef::String(text) => Value::String(text.to_owned()),
        ProjectionValueRef::Enum { variant, .. } => Value::String(variant.to_owned()),
        ProjectionValueRef::List(values) => Value::Array(
            values
                .iter()
                .map(projection_value_to_json)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ProjectionValueRef::Object(fields) => {
            let mut object = serde_json::Map::new();
            for field in fields {
                object.insert(
                    field.name().to_owned(),
                    projection_value_to_json(field.value())?,
                );
            }
            Value::Object(object)
        }
    })
}
