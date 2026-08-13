//! Bind mutation inputs from portable event handlers and adapt to projection IR.

use std::collections::BTreeMap;

use crate::projection::{
    ProjectionArm, ProjectionEventSelector, ProjectionExpression, ProjectionPartition,
    ProjectionProgram, ProjectionValueType,
};

use super::expression::MutationExpression;
use super::program::MutationProgram;
use super::MutationProgramError;

/// One pure binding from an event occurrence into a mutation input path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationInputBinding {
    /// Mutation input path (e.g. `["todo"]` or `["todo_id"]`).
    path: Vec<String>,
    /// Projection expression evaluated against the event occurrence.
    expression: ProjectionExpression,
}

impl MutationInputBinding {
    /// Construct one input binding.
    ///
    /// # Errors
    ///
    /// Rejects an empty path.
    pub fn try_new(
        path: Vec<String>,
        expression: ProjectionExpression,
    ) -> Result<Self, MutationProgramError> {
        if path.is_empty() || path.iter().any(|segment| segment.is_empty()) {
            return Err(MutationProgramError::EmptyName("mutation input path"));
        }
        Ok(Self { path, expression })
    }

    /// Return the mutation input path.
    pub fn path(&self) -> &[String] {
        &self.path
    }

    /// Return the event-side expression.
    pub fn expression(&self) -> &ProjectionExpression {
        &self.expression
    }
}

/// Compiler-visible binding of one event contract to one mutation program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationEventBinding {
    /// Exact event selector.
    selector: ProjectionEventSelector,
    /// Ordered mutation input bindings.
    inputs: Vec<MutationInputBinding>,
    /// Event-independent mutation program.
    program: MutationProgram,
}

impl MutationEventBinding {
    /// Construct a validated event-to-mutation binding.
    ///
    /// # Errors
    ///
    /// Rejects duplicate input paths.
    pub fn try_new(
        selector: ProjectionEventSelector,
        inputs: Vec<MutationInputBinding>,
        program: MutationProgram,
    ) -> Result<Self, MutationProgramError> {
        let mut seen = std::collections::BTreeSet::new();
        for input in &inputs {
            let key = input.path.join(".");
            if !seen.insert(key.clone()) {
                return Err(MutationProgramError::DuplicateName {
                    kind: "mutation input binding",
                    name: key,
                });
            }
        }
        Ok(Self {
            selector,
            inputs,
            program,
        })
    }

    /// Return the exact event selector.
    pub fn selector(&self) -> &ProjectionEventSelector {
        &self.selector
    }

    /// Return input bindings.
    pub fn inputs(&self) -> &[MutationInputBinding] {
        &self.inputs
    }

    /// Return the bound mutation program.
    pub fn program(&self) -> &MutationProgram {
        &self.program
    }

    /// Materialize a temporary event-coupled projection arm for the internal projection mount
    /// execution against the existing projection interpreter.
    ///
    /// # Errors
    ///
    /// Propagates rewrite and projection validation failures.
    pub fn to_projection_arm(
        &self,
        arm_id: impl Into<String>,
    ) -> Result<ProjectionArm, MutationProgramError> {
        let binder = InputPathBinder::new(&self.inputs);
        let operations = self
            .program
            .rewrite_to_projection_operations(&|path, value_type| binder.bind(path, value_type))?;
        ProjectionArm::try_new(arm_id.into(), self.selector.clone(), operations).map_err(Into::into)
    }

    /// Materialize a temporary event-coupled projection program for the internal projection mount
    /// execution. The projection program name and version are taken from the
    /// mutation program; partition defaults to unit unless overridden.
    ///
    /// # Errors
    ///
    /// Propagates rewrite and projection validation failures.
    pub fn to_projection_program(
        &self,
        projection_name: impl Into<String>,
        version: u64,
        partition: ProjectionPartition,
        arm_id: impl Into<String>,
    ) -> Result<ProjectionProgram, MutationProgramError> {
        let arm = self.to_projection_arm(arm_id)?;
        ProjectionProgram::try_new(projection_name, version, partition, vec![arm])
            .map_err(Into::into)
    }
}

struct InputPathBinder<'a> {
    /// Map from first path segment (and optionally nested) to expressions.
    by_path: BTreeMap<String, &'a ProjectionExpression>,
}

impl<'a> InputPathBinder<'a> {
    fn new(inputs: &'a [MutationInputBinding]) -> Self {
        let mut by_path = BTreeMap::new();
        for input in inputs {
            by_path.insert(input.path.join("."), &input.expression);
        }
        Self { by_path }
    }

    fn bind(
        &self,
        path: &[String],
        value_type: &ProjectionValueType,
    ) -> Result<ProjectionExpression, MutationProgramError> {
        let exact = path.join(".");
        if let Some(expression) = self.by_path.get(&exact) {
            return Ok((*expression).clone());
        }
        // Support object-root bindings: input.todo + field path todo.title
        // becomes body_path extension when the root binding is a body path.
        if path.len() > 1 {
            let root = path[0].clone();
            if let Some(root_expression) = self.by_path.get(&root) {
                return extend_expression(root_expression, &path[1..], value_type);
            }
        }
        // Allow single-segment roots that match the first path component of a
        // multi-segment mutation path when an object root was bound.
        if !path.is_empty() {
            for (bound_path, expression) in &self.by_path {
                if bound_path == &path[0] {
                    if path.len() == 1 {
                        return Ok((*expression).clone());
                    }
                    return extend_expression(expression, &path[1..], value_type);
                }
            }
        }
        Err(MutationProgramError::MissingInput {
            path: path.join("."),
        })
    }
}

fn extend_expression(
    root: &ProjectionExpression,
    suffix: &[String],
    value_type: &ProjectionValueType,
) -> Result<ProjectionExpression, MutationProgramError> {
    use crate::projection::ProjectionExpression as PE;
    // Prefer composing body_path when the root is itself a body path.
    // For other roots, fall back to requiring exact leaf bindings.
    if suffix.is_empty() {
        return Ok(root.clone());
    }
    // Attempt to recover path from serialization shape is fragile; instead
    // require callers to bind full leaf paths OR bind a state object root and
    // we synthesize body paths when the root expression is body_path via a
    // well-known helper.
    if let Some(base_path) = body_path_segments(root) {
        let mut path = base_path;
        path.extend(suffix.iter().cloned());
        return PE::body_path(value_type.clone(), path).map_err(Into::into);
    }
    Err(MutationProgramError::MissingInput {
        path: suffix.join("."),
    })
}

fn body_path_segments(expression: &ProjectionExpression) -> Option<Vec<String>> {
    // ProjectionExpression does not expose body path publicly beyond as_ref which is crate-private.
    // Use JSON round-trip of the public Serialize form.
    let value = serde_json::to_value(expression).ok()?;
    if value.get("kind")?.as_str()? != "body_path" {
        return None;
    }
    let path = value
        .get("path")?
        .as_array()?
        .iter()
        .map(|segment| segment.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    Some(path)
}

/// Build a trivial identity binder where each mutation input path is already a
/// projection body path with the same segments (useful for tests).
pub fn identity_body_path_binder(
    path: &[String],
    value_type: &ProjectionValueType,
) -> Result<ProjectionExpression, MutationProgramError> {
    ProjectionExpression::body_path(value_type.clone(), path.iter().cloned()).map_err(Into::into)
}

/// Construct a simple field binding: mutation input path <- event body path.
pub fn body_field_binding(
    input_path: impl IntoIterator<Item = impl Into<String>>,
    body_path: impl IntoIterator<Item = impl Into<String>>,
    value_type: ProjectionValueType,
) -> Result<MutationInputBinding, MutationProgramError> {
    let path = input_path.into_iter().map(Into::into).collect::<Vec<_>>();
    let expression = ProjectionExpression::body_path(value_type, body_path)
        .map_err(MutationProgramError::from)?;
    MutationInputBinding::try_new(path, expression)
}

/// Construct a binding from a mutation input path to an envelope field.
pub fn envelope_binding(
    input_path: impl IntoIterator<Item = impl Into<String>>,
    field: crate::projection::ProjectionEnvelopeField,
) -> Result<MutationInputBinding, MutationProgramError> {
    let path = input_path.into_iter().map(Into::into).collect::<Vec<_>>();
    MutationInputBinding::try_new(path, ProjectionExpression::envelope(field))
}

/// Collect required input root paths from a mutation program for catalog digests.
pub fn required_input_paths(program: &MutationProgram) -> Vec<Vec<String>> {
    let mut paths = std::collections::BTreeSet::new();
    for operation in program.operations() {
        for field in operation.key() {
            if let Some((path, _)) = field.expression().as_input_path() {
                paths.insert(path.to_vec());
            }
        }
        for field in operation.fields() {
            if let super::expression::MutationAssignment::Set(expression) = field.assignment() {
                if let Some((path, _)) = expression.as_input_path() {
                    paths.insert(path.to_vec());
                }
            }
        }
    }
    paths.into_iter().collect()
}

// Silence unused import warning when MutationExpression is only used via paths.
#[allow(dead_code)]
fn _use_mutation_expression(_: &MutationExpression) {}
