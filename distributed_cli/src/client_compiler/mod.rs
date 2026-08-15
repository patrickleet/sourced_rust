//! Pure, framework-neutral compiler for Distributed client artifacts.
//!
//! This module deliberately owns no filesystem or glob behavior. Callers load
//! one role/application-selected manifest and the colocated GraphQL documents,
//! then decide how to write or check the returned files.

mod command_manifest;
mod graphql;
mod manifest;
mod projection_delta;
mod render;

#[cfg(test)]
mod command_manifest_tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use graphql::{compile_document, CompiledOperation};
use manifest::ClientManifest;

pub(crate) fn expected_protocol_fingerprint() -> Result<String, ClientCompileError> {
    manifest::protocol_fingerprint()
}

pub(crate) fn expected_manifest_version() -> u64 {
    manifest::expected_manifest_version()
}
use render::render_project;

/// Complete input to one deterministic client compilation.
#[derive(Clone, Debug)]
pub struct ClientCompileInput {
    /// The role/application-selected Distributed client manifest.
    pub manifest: JsonValue,
    /// Exact authorization surface expected by the caller.
    pub selector: ClientSurfaceSelector,
    /// GraphQL sources. Filesystem traversal and glob expansion stay in the CLI.
    pub documents: Vec<ClientDocument>,
    /// Explicit fallback for `@load` documents outside the conventional route
    /// location. Equivalent CLI syntax is `--route Operation=/route-id`.
    pub route_registrations: Vec<ClientRouteRegistration>,
}

impl ClientCompileInput {
    pub fn new(
        manifest: JsonValue,
        selector: ClientSurfaceSelector,
        documents: Vec<ClientDocument>,
    ) -> Self {
        Self {
            manifest,
            selector,
            documents,
            route_registrations: Vec::new(),
        }
    }

    pub fn with_route_registrations(
        mut self,
        route_registrations: Vec<ClientRouteRegistration>,
    ) -> Self {
        self.route_registrations = route_registrations;
        self
    }
}

/// Explicit client authorization surface. This value only verifies manifest
/// provenance; it can never relabel a broader manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientSurfaceSelector {
    Role { name: String },
    Application {
        name: String,
        eligible_roles: Vec<String>,
        schema_roles: Vec<String>,
    },
}

impl ClientSurfaceSelector {
    pub fn role(name: impl Into<String>) -> Self {
        Self::Role { name: name.into() }
    }

    pub fn application(
        name: impl Into<String>,
        eligible_roles: impl IntoIterator<Item = impl Into<String>>,
        schema_roles: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        let mut eligible_roles = eligible_roles.into_iter().map(Into::into).collect::<Vec<_>>();
        let mut schema_roles = schema_roles.into_iter().map(Into::into).collect::<Vec<_>>();
        eligible_roles.sort();
        eligible_roles.dedup();
        schema_roles.sort();
        schema_roles.dedup();
        Self::Application {
            name: name.into(),
            eligible_roles,
            schema_roles,
        }
    }
}

/// One already-loaded GraphQL source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientDocument {
    pub path: String,
    pub source: String,
}

impl ClientDocument {
    pub fn new(path: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            source: source.into(),
        }
    }
}

/// Explicit `@load` route fallback, keyed by the GraphQL operation name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientRouteRegistration {
    pub operation: String,
    pub route: String,
}

impl ClientRouteRegistration {
    pub fn new(operation: impl Into<String>, route: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            route: route.into(),
        }
    }
}

/// Pure compiler output. `files` is sorted by portable relative path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedClientProject {
    pub files: Vec<GeneratedClientFile>,
    pub operations: Vec<GeneratedOperationSummary>,
    pub routes: Vec<GeneratedRoutePlan>,
    pub schema_fingerprint: String,
    pub protocol_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedClientFile {
    pub path: String,
    pub contents: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GeneratedOperationSummary {
    pub name: String,
    pub source_path: String,
    pub module_path: String,
    pub export_name: String,
    pub operation_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub live_operation_hash: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct GeneratedRoutePlan {
    pub operation: String,
    pub route: String,
    pub source_path: String,
    pub discovery: ClientRouteDiscovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientRouteDiscovery {
    Convention,
    Explicit,
}

/// Stable, source-located error returned for every unsupported or invalid
/// construct. The first compiler slice never silently weakens an operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientCompileError {
    pub code: &'static str,
    pub message: String,
    pub source: Option<ClientSourceLocation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientSourceLocation {
    pub path: String,
    pub line: usize,
    pub column: usize,
}

impl ClientCompileError {
    pub(crate) fn manifest(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    pub(crate) fn source(
        code: &'static str,
        message: impl Into<String>,
        path: &str,
        line: usize,
        column: usize,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            source: Some(ClientSourceLocation {
                path: path.to_string(),
                line,
                column,
            }),
        }
    }
}

impl fmt::Display for ClientCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(source) = &self.source {
            write!(
                formatter,
                "{}:{}:{}: {} [{}]",
                source.path, source.line, source.column, self.message, self.code
            )
        } else {
            write!(formatter, "{} [{}]", self.message, self.code)
        }
    }
}

impl std::error::Error for ClientCompileError {}

/// Compile one selected manifest and its colocated GraphQL sources.
pub fn compile_client(
    mut input: ClientCompileInput,
) -> Result<GeneratedClientProject, ClientCompileError> {
    let manifest = ClientManifest::parse(input.manifest, &input.selector)?;

    if input.documents.is_empty() {
        return Err(ClientCompileError::manifest(
            "client.documents.empty",
            "client compilation requires at least one GraphQL document",
        ));
    }

    for document in &mut input.documents {
        document.path = normalize_source_path(&document.path)?;
    }
    input.documents.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.source.cmp(&right.source))
    });
    for pair in input.documents.windows(2) {
        if pair[0].path == pair[1].path {
            return Err(ClientCompileError::manifest(
                "client.documents.duplicate_path",
                format!("duplicate GraphQL document path `{}`", pair[0].path),
            ));
        }
    }

    let registrations = validate_route_registrations(input.route_registrations)?;
    let mut used_registrations = BTreeSet::new();
    let mut operations = Vec::with_capacity(input.documents.len());
    for document in &input.documents {
        let operation = compile_document(document, &manifest, &registrations)?;
        if operation.route.as_ref().is_some_and(|route| {
            route.discovery == ClientRouteDiscovery::Explicit
                && used_registrations.insert(operation.name.clone())
        }) {
            // Insert performed by the predicate.
        }
        operations.push(operation);
    }

    operations.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| left.source_path.cmp(&right.source_path))
    });
    let mut names = BTreeMap::<&str, &str>::new();
    let mut modules = BTreeMap::<&str, &str>::new();
    for operation in &operations {
        if let Some(previous_path) =
            names.insert(operation.name.as_str(), operation.source_path.as_str())
        {
            return Err(ClientCompileError::manifest(
                "client.operation.duplicate_name",
                format!(
                    "duplicate GraphQL operation `{}` in `{}` and `{}`",
                    operation.name, previous_path, operation.source_path
                ),
            ));
        }
        if let Some(previous_name) =
            modules.insert(operation.module_path.as_str(), operation.name.as_str())
        {
            return Err(ClientCompileError::manifest(
                "client.operation.module_collision",
                format!(
                    "operations `{}` and `{}` collide at generated module `{}`",
                    previous_name, operation.name, operation.module_path
                ),
            ));
        }
    }

    for operation in registrations.keys() {
        if !operations
            .iter()
            .any(|candidate| &candidate.name == operation)
        {
            return Err(ClientCompileError::manifest(
                "client.route.unknown_registration",
                format!("explicit route registration names unknown operation `{operation}`"),
            ));
        }
        if !used_registrations.contains(operation) {
            return Err(ClientCompileError::manifest(
                "client.route.unused_registration",
                format!(
                    "explicit route registration for `{operation}` is unused; registrations are only valid as the `@load` fallback"
                ),
            ));
        }
    }

    validate_unique_routes(&operations)?;
    render_project(&manifest, operations)
}

fn normalize_source_path(path: &str) -> Result<String, ClientCompileError> {
    let normalized = path.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Err(ClientCompileError::manifest(
            "client.documents.empty_path",
            "GraphQL document path must not be empty",
        ));
    }
    if normalized.split('/').any(|segment| segment == "..") {
        return Err(ClientCompileError::manifest(
            "client.documents.parent_path",
            format!("GraphQL document path `{path}` must not contain `..`"),
        ));
    }
    Ok(normalized)
}

fn validate_route_registrations(
    mut registrations: Vec<ClientRouteRegistration>,
) -> Result<BTreeMap<String, String>, ClientCompileError> {
    registrations.sort_by(|left, right| {
        left.operation
            .cmp(&right.operation)
            .then_with(|| left.route.cmp(&right.route))
    });
    let mut result = BTreeMap::new();
    for registration in registrations {
        if !is_graphql_name(&registration.operation) {
            return Err(ClientCompileError::manifest(
                "client.route.invalid_operation",
                format!(
                    "route registration operation `{}` is not a GraphQL name",
                    registration.operation
                ),
            ));
        }
        let route = normalize_route(&registration.route)?;
        if result
            .insert(registration.operation.clone(), route)
            .is_some()
        {
            return Err(ClientCompileError::manifest(
                "client.route.duplicate_registration",
                format!(
                    "duplicate route registration for operation `{}`",
                    registration.operation
                ),
            ));
        }
    }
    Ok(result)
}

fn normalize_route(route: &str) -> Result<String, ClientCompileError> {
    let mut normalized = route.trim().replace('\\', "/");
    if normalized.is_empty() || !normalized.starts_with('/') {
        return Err(ClientCompileError::manifest(
            "client.route.invalid",
            format!("route `{route}` must be non-empty and start with `/`"),
        ));
    }
    if normalized.contains('?')
        || normalized.contains('#')
        || normalized.split('/').any(|segment| segment == "..")
    {
        return Err(ClientCompileError::manifest(
            "client.route.invalid",
            format!("route `{route}` must not contain query, fragment, or parent segments"),
        ));
    }
    while normalized.len() > 1 && normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

fn validate_unique_routes(operations: &[CompiledOperation]) -> Result<(), ClientCompileError> {
    let mut routes = BTreeMap::<&str, &str>::new();
    for operation in operations {
        let Some(route) = &operation.route else {
            continue;
        };
        if let Some(previous) = routes.insert(&route.route, &operation.name) {
            return Err(ClientCompileError::manifest(
                "client.route.duplicate",
                format!(
                    "route `{}` is owned by both `{}` and `{}`",
                    route.route, previous, operation.name
                ),
            ));
        }
    }
    Ok(())
}

pub(crate) fn is_graphql_name(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some('_' | 'A'..='Z' | 'a'..='z'))
        && chars.all(|character| matches!(character, '_' | '0'..='9' | 'A'..='Z' | 'a'..='z'))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod runtime_bridge_tests;
