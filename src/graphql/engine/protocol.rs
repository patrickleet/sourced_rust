use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RequestedProtocolClient {
    surface: ClientSurfaceIdentity,
    schema_hash: String,
}

fn requested_protocol_client(request: &Request) -> Result<Option<RequestedProtocolClient>, ()> {
    let Some(distributed) = request.extensions.get("distributed") else {
        return Ok(None);
    };
    let distributed = distributed.clone().into_json().map_err(|_| ())?;
    let distributed = distributed.as_object().ok_or(())?;
    let Some(client) = distributed.get("client") else {
        return Ok(None);
    };
    serde_json::from_value(client.clone())
        .map(Some)
        .map_err(|_| ())
}

/// Asserted engine roles for this principal: primary `role` plus `x-roles`.
pub(crate) fn principal_asserted_roles(session: &Session, primary_role: &str) -> Vec<String> {
    let mut roles = Vec::new();
    if !primary_role.is_empty() {
        roles.push(primary_role.to_string());
    }
    if let Some(raw) = session
        .get("x-roles")
        .or_else(|| session.get("X-Roles"))
    {
        for part in raw.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if !roles.iter().any(|existing| existing == part) {
                roles.push(part.to_string());
            }
        }
    }
    roles
}

fn principal_may_open_application(asserted: &[String], eligible: &[String]) -> bool {
    asserted.iter().any(|role| {
        eligible
            .binary_search_by(|candidate| candidate.as_str().cmp(role.as_str()))
            .is_ok()
    })
}

pub(crate) fn select_protocol_surface<'a>(
    runtime: &'a ProtocolRuntime,
    role: &str,
    session: &Session,
    request: &Request,
) -> Result<(ClientSurfaceIdentity, &'a ProtocolSurfaceInfo), ()> {
    let Some(requested) = requested_protocol_client(request)? else {
        let info = runtime.roles.get(role).ok_or(())?;
        return Ok((ClientSurfaceIdentity::role(role), &info.surface));
    };
    match requested.surface {
        ClientSurfaceIdentity::Role { name } => {
            if name != role {
                return Err(());
            }
            let info = runtime.roles.get(&name).ok_or(())?;
            if requested.schema_hash != info.surface.schema_fingerprint {
                return Err(());
            }
            Ok((ClientSurfaceIdentity::role(name), &info.surface))
        }
        ClientSurfaceIdentity::Application { name, roles } => {
            let application = runtime.applications.get(&name).ok_or(())?;
            // Wire roles must equal the registered eligible set (canonical).
            // Principal may open when any asserted role is eligible — not only
            // when the primary engine role is the sole surface role.
            let asserted = principal_asserted_roles(session, role);
            if roles != application.roles
                || !principal_may_open_application(&asserted, &application.roles)
                || requested.schema_hash != application.surface.schema_fingerprint
            {
                return Err(());
            }
            Ok((
                ClientSurfaceIdentity::application(name, roles),
                &application.surface,
            ))
        }
    }
}

pub(crate) fn resolve_protocol_preset(
    session: &Session,
    descriptor: &ClientTrustedPresetDescriptor,
) -> Option<DistributedTrustedPreset> {
    use base64::Engine as _;

    // Match SQL row-policy claim lookup exactly: applications commonly
    // normalize HTTP header names to lowercase before constructing Session.
    let raw = session
        .get(&descriptor.name)
        .or_else(|| session.get(&descriptor.name.to_ascii_lowercase()))?;
    let value = match descriptor.codec.as_str() {
        "string" | "string_unvalidated_timestamp" => serde_json::Value::String(raw.to_string()),
        "base64" => {
            let decoded = base64::engine::general_purpose::STANDARD.decode(raw).ok()?;
            if base64::engine::general_purpose::STANDARD.encode(decoded) != raw {
                return None;
            }
            serde_json::Value::String(raw.to_string())
        }
        "boolean" => match raw {
            "true" => serde_json::Value::Bool(true),
            "false" => serde_json::Value::Bool(false),
            _ => return None,
        },
        "int32" => {
            let parsed = raw.parse::<i32>().ok()?;
            if parsed.to_string() != raw {
                return None;
            }
            serde_json::Value::Number(parsed.into())
        }
        "json_number_precision_limited" => {
            let parsed = raw.parse::<i64>().ok()?;
            if !(-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&parsed)
                || parsed.to_string() != raw
            {
                return None;
            }
            serde_json::Value::Number(parsed.into())
        }
        "float64" => {
            let parsed = raw.parse::<f64>().ok()?;
            if !parsed.is_finite() {
                return None;
            }
            serde_json::Value::Number(serde_json::Number::from_f64(parsed)?)
        }
        "json" => serde_json::from_str(raw).ok()?,
        _ => return None,
    };
    Some(DistributedTrustedPreset {
        name: descriptor.name.clone(),
        codec: descriptor.codec.clone(),
        value,
    })
}

pub(crate) fn protocol_trusted_presets(
    manifest: &DistributedClientManifest,
) -> Result<Vec<ClientTrustedPresetDescriptor>, GraphqlBuildError> {
    trusted_preset_descriptors(manifest).map_err(|error| GraphqlBuildError(error.to_string()))
}

pub(crate) fn operation_fingerprint(document: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(document.as_bytes()))
}

/// Until the query executor owns an operation-wide database transaction, two
/// independent read roots cannot truthfully share one causal snapshot. Fail
/// closed instead of merging separately observed rows and duplicate index
/// vectors into an envelope that generated clients would treat as atomic.
pub(crate) fn has_multiple_protocol_query_roots(
    inner: &EngineInner,
    role: &str,
    request: &mut Request,
) -> bool {
    if inner.protocol.is_none() {
        return false;
    }
    let Some(surface) = inner.role_surfaces.get(role) else {
        return false;
    };
    let query_roots = surface
        .query_root_names()
        .into_iter()
        .collect::<BTreeSet<_>>();
    if query_roots.is_empty() {
        return false;
    }

    let operation_name = request.operation_name.clone();
    let Ok(document) = request.parsed_query() else {
        return false;
    };
    let mut operations = document.operations.iter();
    let operation = if let Some(requested) = operation_name.as_deref() {
        operations
            .find(|(name, _)| name.map(|name| name.as_str()) == Some(requested))
            .map(|(_, operation)| operation)
    } else {
        let first = operations.next().map(|(_, operation)| operation);
        if operations.next().is_some() {
            None
        } else {
            first
        }
    };
    let Some(operation) = operation else {
        return false;
    };
    if operation.node.ty != async_graphql::parser::types::OperationType::Query {
        return false;
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "bounded recursive request analysis threads explicit cycle, budget, and result state"
    )]
    fn collect_root_keys<'a>(
        selection: &'a async_graphql::parser::types::SelectionSet,
        document: &'a async_graphql::parser::types::ExecutableDocument,
        query_roots: &BTreeSet<&str>,
        visiting: &mut BTreeSet<String>,
        completed: &mut BTreeSet<String>,
        remaining_selections: &mut usize,
        response_keys: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<(), ()> {
        if response_keys.len() > 1 {
            return Ok(());
        }
        if depth > REQUEST_ANALYSIS_MAX_DEPTH || selection.items.len() > *remaining_selections {
            return Err(());
        }
        *remaining_selections -= selection.items.len();
        for item in &selection.items {
            match &item.node {
                async_graphql::parser::types::Selection::Field(field) => {
                    if query_roots.contains(field.node.name.node.as_str()) {
                        response_keys.insert(field.node.response_key().node.to_string());
                    }
                }
                async_graphql::parser::types::Selection::InlineFragment(fragment) => {
                    collect_root_keys(
                        &fragment.node.selection_set.node,
                        document,
                        query_roots,
                        visiting,
                        completed,
                        remaining_selections,
                        response_keys,
                        depth + 1,
                    )?;
                }
                async_graphql::parser::types::Selection::FragmentSpread(spread) => {
                    let name = spread.node.fragment_name.node.to_string();
                    if completed.contains(&name) {
                        continue;
                    }
                    if !visiting.insert(name.clone()) {
                        return Err(());
                    }
                    let Some(fragment) = document.fragments.get(&spread.node.fragment_name.node)
                    else {
                        return Err(());
                    };
                    let result = collect_root_keys(
                        &fragment.node.selection_set.node,
                        document,
                        query_roots,
                        visiting,
                        completed,
                        remaining_selections,
                        response_keys,
                        depth + 1,
                    );
                    visiting.remove(&name);
                    result?;
                    completed.insert(name);
                }
            }
            if response_keys.len() > 1 {
                return Ok(());
            }
        }
        Ok(())
    }

    let mut response_keys = BTreeSet::new();
    let mut remaining_selections = REQUEST_ANALYSIS_MAX_SELECTIONS;
    let analysis = collect_root_keys(
        &operation.node.selection_set.node,
        document,
        &query_roots,
        &mut BTreeSet::new(),
        &mut BTreeSet::new(),
        &mut remaining_selections,
        &mut response_keys,
        0,
    );
    analysis.is_err() || response_keys.len() > 1
}

pub(crate) fn protocol_multi_root_error_response() -> Response {
    Response::from_errors(vec![ServerError::new(
        "causal GraphQL operations currently support one read root so data and revision evidence share one atomic snapshot; split this operation into separate requests",
        None,
    )])
}
