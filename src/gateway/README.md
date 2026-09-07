# Gateway identity boundary

`AuthProvider` validates current `Credentials` and supplies `RequestContext` for
UI, API, assets and custom routes. Run authentication before each consumer's
route admission or delivery reuse; an expired assertion fails even on a public
route. `provider_revision` changes when provider policy/configuration changes.
An application can replace the provider without changing route declarations.
Contexts are in-process assertions and have no client deserializer.

`BackendCredential` is explicit: either none or a provider-supplied bearer.
The gateway does not translate a cookie or `x-user-id` into a credential.
A configured session provider may resolve an Auth.js session and supply its
access token; the backend must still validate that token and authorize the
operation. With `graphql` enabled, `graphql::identity::OidcGatewayProvider`
reuses the existing JWT validator, JWKS cache and role mapping. This optional
adapter does not make the portable gateway depend on GraphQL.

Delegate login/callback/refresh/logout to existing auth handlers. Configure
`AUTH_URL` as the public origin. Strip incoming identity and forwarded headers,
including deployment-specific identity/secret names, before adding trusted
public-origin metadata. Preserve Origin for CSRF and every Set-Cookie header.
Credentials are redacted from Debug output and are not cache keys.

The reusable production Auth.js/OIDC/browser fixture is in
`tests/gateway-auth`. Network adapters and application mounts own transport
plumbing; merely declaring routes or providers starts no service or identity
store. Removing a gateway mount restores the application's prior entrypoints;
backend authentication stays enabled.
