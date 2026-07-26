use super::*;

impl GraphqlEngine {
    pub async fn execute(&self, session: &Session, mut request: Request) -> Response {
        let role = resolve_role(session, &self.inner.anonymous_role);
        let introspection = self.inner.graphiql && is_pure_introspection_request(&mut request);
        let schema = if introspection {
            self.inner
                .graphiql_schemas
                .get(&role)
                .or_else(|| self.inner.schemas.get(&role))
        } else {
            self.inner.schemas.get(&role)
        };
        let Some(schema) = schema else {
            return Response::from_errors(vec![ServerError::new(
                format!("role `{role}` is not configured for GraphQL"),
                None,
            )]);
        };
        if has_multiple_protocol_query_roots(&self.inner, &role, &mut request) {
            return protocol_multi_root_error_response();
        }

        let accumulator = match self.protocol_accumulator(&role, session, &request) {
            Ok(accumulator) => accumulator,
            Err(()) => return protocol_internal_error_response(),
        };
        if introspection {
            // The relaxed schema is defense-in-depth restricted even if a
            // future classifier or request extension behaves unexpectedly.
            request = request.only_introspection();
        }
        let mut request = request.data(session.clone()).data(Arc::clone(&self.inner));
        if let Some(accumulator) = &accumulator {
            request = request.data(accumulator.clone());
        }
        let start = std::time::Instant::now();
        let response =
            attach_protocol_response(schema.execute(request).await, accumulator.as_ref());
        let status = metrics_status_for_response(&response);
        let root_field = match &response.data {
            Value::Object(map) => map.keys().next().map(|s| s.as_str()).unwrap_or("_"),
            _ => "_",
        };
        record_metrics(session, root_field, status, start.elapsed());
        response
    }

    /// Execute a GraphQL subscription document as a stream of responses.
    pub fn execute_stream(
        &self,
        session: &Session,
        mut request: Request,
    ) -> BoxStream<'static, async_graphql::Response> {
        let role = resolve_role(session, &self.inner.anonymous_role);
        let introspection = self.inner.graphiql && is_pure_introspection_request(&mut request);
        let schema = if introspection {
            self.inner
                .graphiql_schemas
                .get(&role)
                .or_else(|| self.inner.schemas.get(&role))
        } else {
            self.inner.schemas.get(&role)
        };
        let Some(schema) = schema.cloned() else {
            return stream::once(async move {
                Response::from_errors(vec![ServerError::new(
                    format!("role `{role}` is not configured for GraphQL"),
                    None,
                )])
            })
            .boxed();
        };
        if has_multiple_protocol_query_roots(&self.inner, &role, &mut request) {
            return stream::once(async { protocol_multi_root_error_response() }).boxed();
        }
        let accumulator = match self.protocol_accumulator(&role, session, &request) {
            Ok(accumulator) => accumulator,
            Err(()) => {
                return stream::once(async { protocol_internal_error_response() }).boxed();
            }
        };
        if accumulator
            .as_ref()
            .is_some_and(|accumulator| accumulator.begin_stream().is_err())
        {
            return stream::once(async { protocol_internal_error_response() }).boxed();
        }
        if introspection {
            request = request.only_introspection();
        }
        let mut request = request
            .data(session.clone())
            .data(std::sync::Arc::clone(&self.inner));
        if let Some(accumulator) = &accumulator {
            request = request.data(accumulator.clone());
        }
        schema
            .execute_stream(request)
            .map(move |response| attach_protocol_response(response, accumulator.as_ref()))
            .boxed()
    }

    fn protocol_accumulator(
        &self,
        role: &str,
        session: &Session,
        request: &Request,
    ) -> Result<Option<ProtocolResponseAccumulator>, ()> {
        let Some(runtime) = &self.inner.protocol else {
            return Ok(None);
        };
        let role_info = runtime.roles.get(role).ok_or(())?;
        let (surface_identity, surface_info) =
            select_protocol_surface(runtime, role, request).map_err(|_| ())?;
        let trusted_presets = surface_info
            .trusted_presets
            .iter()
            .map(|descriptor| resolve_protocol_preset(session, descriptor).ok_or(()))
            .collect::<Result<Vec<_>, _>>()?;
        let principal = request
            .data
            .get(&TypeId::of::<VerifiedPrincipal>())
            .and_then(|principal| principal.downcast_ref::<VerifiedPrincipal>());
        let principal_partition =
            principal.map(|principal| principal.partition_for_service(&runtime.service_id));
        let session_authorization_context = role_info
            .claim_keys
            .iter()
            .map(|key| (key.as_str(), session.get(key)))
            .collect::<Vec<_>>();

        #[derive(Serialize)]
        struct CacheScopeMaterial<'a> {
            domain: &'static str,
            version: u32,
            namespace: &'a str,
            service_id: &'a str,
            role: &'a str,
            surface: &'a ClientSurfaceIdentity,
            schema_fingerprint: &'a str,
            protocol_fingerprint: &'a str,
            authorization_surface_fingerprint: &'a str,
            identity_mode: &'static str,
            verified_principal_partition: Option<&'a str>,
            session_authorization_context: Vec<(&'a str, Option<&'a str>)>,
            trusted_presets: &'a [DistributedTrustedPreset],
        }

        // Only session values that can affect authorization enter the HMAC:
        // role/user plus claim keys referenced by this role's row policies.
        // Ambient headers such as cookies or user-agent must not churn caches.
        // Raw values and the verified principal partition remain private HMAC
        // inputs and are never echoed in the response.
        let material = CacheScopeMaterial {
            domain: "distributed.graphql.cache-scope",
            version: 1,
            namespace: &runtime.namespace,
            service_id: &runtime.service_id,
            role,
            surface: &surface_identity,
            schema_fingerprint: &surface_info.schema_fingerprint,
            protocol_fingerprint: &surface_info.protocol_fingerprint,
            authorization_surface_fingerprint: &role_info.authorization_fingerprint,
            identity_mode: identity_mode_label(self.inner.identity.mode),
            verified_principal_partition: principal_partition.as_deref(),
            session_authorization_context,
            trusted_presets: &trusted_presets,
        };
        let cache_scope = runtime
            .codec
            .issue(ProtocolTokenPurpose::CacheScope, &material)
            .map_err(|_| ())?;
        let envelope = DistributedEnvelopeV2::new(
            surface_info.schema_fingerprint.clone(),
            cache_scope,
            // Generated artifacts submit this exact document. Hashing its
            // bytes matches manifest operation_hash and provides a useful
            // identity/drift fence without claiming APQ negotiation.
            Some(operation_fingerprint(&request.query)),
        )
        .with_trusted_presets(trusted_presets);
        let accumulator = ProtocolResponseAccumulator::new(envelope, runtime.codec.clone());
        accumulator
            .set_requested_live_resume(parse_requested_live_resume(request))
            .map_err(|_| ())?;
        Ok(Some(accumulator))
    }
}
